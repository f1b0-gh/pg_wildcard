// Copyright 2026 The pg_wildcard Authors
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use super::ChError;

/// Maximum escaped path guard size; exact row predicates are never removed.
pub const MAX_LAKE_FILTER_BYTES: usize = 64 * 1024;

/// Why a lake guard was selected, or why pruning was disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LakeFilterStrategy {
    ExcludedInline,
    KnownInventory,
    NoExclusions,
    FilterBudgetExceeded,
}

impl LakeFilterStrategy {
    pub fn description(self) -> &'static str {
        match self {
            Self::ExcludedInline => "excluded paths inline",
            Self::KnownInventory => "server inventory with immutable coverage binding",
            Self::NoExclusions => "no indexed files excluded",
            Self::FilterBudgetExceeded => "pruning disabled: escaped path guard exceeds 64 KiB",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LakePathFilter {
    pub condition: Option<String>,
    pub strategy: LakeFilterStrategy,
}

/// Content-address the exact inventory, independent of ordering and duplicates.
///
/// The versioned domain and length framing disambiguate arbitrary UTF-8 paths.
/// Remote inventory loaders MUST only insert this inventory's paths under its
/// identifier. Missing rows are safe: they widen the scan. Reusing a mutable
/// human index name for the remote key is unsafe and is deliberately unsupported.
pub fn lake_coverage_id(indexed: &[String]) -> String {
    let paths: BTreeSet<_> = indexed.iter().collect();
    let mut hash = Sha256::new();
    hash.update(b"libwildcard/lake-coverage/v1\0");
    hash.update((paths.len() as u64).to_be_bytes());
    for path in paths {
        hash.update((path.len() as u64).to_be_bytes());
        hash.update(path.as_bytes());
    }
    format!("wc-lake-v1:{:x}", hash.finalize())
}

/// Append SQL literals without allocating an oversized intermediate string.
fn append_paths<'a>(
    sql: &mut String,
    paths: impl IntoIterator<Item = &'a String>,
    closing_bytes: usize,
) -> Option<()> {
    for (i, path) in paths.into_iter().enumerate() {
        let separator = if i == 0 { "" } else { ", " };
        let remaining =
            MAX_LAKE_FILTER_BYTES.checked_sub(sql.len() + separator.len() + closing_bytes)?;
        if path.len() > remaining.saturating_sub(2) {
            return None;
        }
        let quoted = super::quote(path);
        if quoted.len() > remaining {
            return None;
        }
        sql.push_str(separator);
        sql.push_str(&quoted);
    }
    Some(())
}

/// Choose the shortest bounded sound path guard.
///
/// `known_table`, when provided, contains `(index_name String, path String)`.
/// Its `index_name` is [`lake_coverage_id`], never the mutable Postgres index
/// name. Populate it using `wc-lake export-pg`'s generated inventory artifacts.
/// The caller authenticates that loader and immutable file paths. Missing rows
/// (including a missing generation) retain unknown files. The table itself must
/// exist; a missing table raises an engine error, never an empty result.
///
/// Candidates must belong to `indexed`. No network calls occur here. Both
/// forms retain all unknown files and require the caller's exact row recheck.
pub fn lake_path_filter_with_inventory(
    indexed: &[String],
    candidates: &[String],
    known_table: Option<&str>,
) -> Result<LakePathFilter, ChError> {
    if let Some(table) = known_table {
        // Bound hostile names before validation/copying into generated SQL.
        if table.len() > 1024 {
            return Err(ChError::Range);
        }
        super::ident(table)?;
    }
    let known: BTreeSet<_> = indexed.iter().collect();
    let chosen: BTreeSet<_> = candidates.iter().collect();
    if !chosen.is_subset(&known) {
        return Err(ChError::Range);
    }
    let excluded: BTreeSet<_> = known.difference(&chosen).copied().collect();
    if excluded.is_empty() {
        return Ok(LakePathFilter {
            condition: None,
            strategy: LakeFilterStrategy::NoExclusions,
        });
    }
    let mut inline = String::from("_path NOT IN (");
    let inline = append_paths(&mut inline, excluded, 1).map(|()| {
        inline.push(')');
        inline
    });
    let remote = known_table.and_then(|table| {
        let mut sql = format!(
            "(_path NOT IN (SELECT path FROM {table} WHERE index_name = {})",
            super::quote(&lake_coverage_id(indexed))
        );
        if !chosen.is_empty() {
            sql.push_str(" OR _path IN (");
            append_paths(&mut sql, chosen, 2)?;
            sql.push(')');
        }
        sql.push(')');
        (sql.len() <= MAX_LAKE_FILTER_BYTES).then_some(sql)
    });
    let (condition, strategy) = match (inline, remote) {
        (Some(inline), Some(remote)) if remote.len() < inline.len() => {
            (Some(remote), LakeFilterStrategy::KnownInventory)
        }
        (Some(inline), _) => (Some(inline), LakeFilterStrategy::ExcludedInline),
        (None, Some(remote)) => (Some(remote), LakeFilterStrategy::KnownInventory),
        (None, None) => (None, LakeFilterStrategy::FilterBudgetExceeded),
    };
    Ok(LakePathFilter {
        condition,
        strategy,
    })
}

/// Exclude only known impossible immutable files, preserving unknown files.
/// Returns no restriction if nothing can be excluded, inputs are inconsistent,
/// or the escaped guard exceeds 64 KiB. For diagnostics or a server inventory,
/// use [`lake_path_filter_with_inventory`].
pub fn lake_path_filter(indexed: &[String], candidates: &[String]) -> Option<String> {
    lake_path_filter_with_inventory(indexed, candidates, None)
        .ok()?
        .condition
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_is_order_independent_and_length_framed() {
        let a = vec!["a".into(), "bc".into()];
        assert_eq!(
            lake_coverage_id(&a),
            lake_coverage_id(&["bc".into(), "a".into(), "a".into()])
        );
        assert_ne!(
            lake_coverage_id(&a),
            lake_coverage_id(&["ab".into(), "c".into()])
        );
        assert_ne!(lake_coverage_id(&a), lake_coverage_id(&["a".into()]));
    }

    #[test]
    fn ten_thousand_paths_choose_compact_safe_shapes() {
        let indexed: Vec<_> = (0..10_000)
            .map(|i| format!("s3://lake/data/{i:08}.parquet"))
            .collect();
        let sparse =
            lake_path_filter_with_inventory(&indexed, &indexed[..1], Some("wc_lake_known"))
                .unwrap();
        assert_eq!(sparse.strategy, LakeFilterStrategy::KnownInventory);
        assert!(sparse.condition.unwrap().len() < 300);
        let dense =
            lake_path_filter_with_inventory(&indexed, &indexed[..9999], Some("wc_lake_known"))
                .unwrap();
        assert_eq!(dense.strategy, LakeFilterStrategy::ExcludedInline);
        assert!(dense.condition.unwrap().len() < 100);
        assert_eq!(
            lake_path_filter_with_inventory(&indexed, &[], None)
                .unwrap()
                .strategy,
            LakeFilterStrategy::FilterBudgetExceeded
        );
        let empty = lake_path_filter_with_inventory(&indexed, &[], Some("wc_lake_known")).unwrap();
        assert_eq!(empty.strategy, LakeFilterStrategy::KnownInventory);
        assert!(!empty.condition.unwrap().contains("OR"));
    }

    #[test]
    fn escaped_budget_and_identifier_validation() {
        let indexed = vec!["'".repeat(40_000), "\\".repeat(40_000)];
        let filter =
            lake_path_filter_with_inventory(&indexed, &indexed[..1], Some("known")).unwrap();
        assert_eq!(filter.strategy, LakeFilterStrategy::FilterBudgetExceeded);
        assert!(filter.condition.is_none());
        assert!(
            lake_path_filter_with_inventory(&indexed, &[], Some("known; DROP TABLE x")).is_err()
        );
        assert!(lake_path_filter_with_inventory(&indexed, &["unknown".into()], None).is_err());
    }
}
