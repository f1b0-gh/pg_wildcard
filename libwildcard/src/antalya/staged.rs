// SPDX-License-Identifier: Apache-2.0
//! Explicit, bounded SQL planning for selective literal staging.
use std::collections::BTreeSet;

use regex_syntax::hir::HirKind;

use super::{ChError, SweepItem, SweepOptions, ViewColumns, ident, quote};
use crate::core::{View, re2};

/// A necessary condition inferred solely from admitted literal patterns.
/// False positives are allowed; every staged query still checks each rule exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiteralGate {
    Prefix(String),
    CharacterMask(String),
}

/// A SQL recipe, not an executor. The caller owns staging and bounded concurrency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedLiteralPlan {
    pub gate: LiteralGate,
    /// Use in CREATE TABLE <staging> ENGINE=Memory AS <candidate_select>.
    /// Includes every projected column and the original raw value, without DISTINCT.
    pub candidate_select: String,
    /// At most eight disjoint 250-rule batches over the caller-named staged table.
    pub queries: Vec<String>,
}

/// Explicit admission policy; there is no automatically enabled default.
/// Value bytes exclude metadata, allocator overhead and query working memory.
/// Enforce engine and process memory limits independently, including acquisition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StagingBudget {
    pub max_rows: u64,
    pub max_value_bytes: u64,
    /// 0..=1000; 100 means at most 10% of the same source population.
    pub max_selectivity_per_mille: u16,
}

impl StagingBudget {
    /// Counts must describe the same source snapshot and its complete candidates.
    /// Refusal means use the existing complete sweep, never truncate candidates.
    pub fn allows(self, source_rows: u64, candidate_rows: u64, value_bytes: u64) -> bool {
        self.max_selectivity_per_mille <= 1000
            && candidate_rows <= source_rows
            && candidate_rows <= self.max_rows
            && value_bytes <= self.max_value_bytes
            && (candidate_rows != 0 || value_bytes == 0)
            && u128::from(candidate_rows) * 1000
                <= u128::from(source_rows) * u128::from(self.max_selectivity_per_mille)
    }
}

fn literal(item: &SweepItem) -> Result<Option<Vec<u8>>, ChError> {
    // Bound parsing before allocation. Unsupported shapes retain the ordinary
    // sweep route; they are never partially staged or silently omitted.
    if item.view != View::Raw || item.regex.len() > 256 {
        return Ok(None);
    }
    let portable = re2::to_rust(&item.regex)?;
    let Ok(hir) = regex_syntax::Parser::new().parse(&portable) else {
        return Ok(None);
    };
    let HirKind::Literal(lit) = hir.kind() else {
        return Ok(None);
    };
    if !(3..=64).contains(&lit.0.len()) || !lit.0.iter().all(|b| b.is_ascii_graphic()) {
        return Ok(None);
    }
    Ok(Some(lit.0.to_vec()))
}

fn necessary_gate(values: &[Vec<u8>]) -> Option<LiteralGate> {
    let first = values.first()?;
    let prefix_len = values.iter().skip(1).fold(first.len(), |n, value| {
        first[..n]
            .iter()
            .zip(value)
            .take_while(|(a, b)| a == b)
            .count()
    });
    if prefix_len >= 3 {
        // Admission proves ASCII. Conversion remains fallible rather than panic.
        return String::from_utf8(first[..prefix_len].to_vec())
            .ok()
            .map(LiteralGate::Prefix);
    }
    if values.iter().any(|v| v.len() != first.len()) {
        return None;
    }
    let mut atoms = Vec::with_capacity(first.len());
    for pos in 0..first.len() {
        let bytes: BTreeSet<_> = values.iter().map(|v| v[pos]).collect();
        // Hex escapes make every admitted punctuation byte safe in a RE2 class.
        let members: String = bytes.iter().map(|b| format!("\\x{b:02X}")).collect();
        atoms.push(if bytes.len() == 1 {
            members
        } else {
            format!("[{members}]")
        });
    }
    let mut mask = String::new();
    let mut start = 0;
    while start < atoms.len() {
        let count = atoms[start..]
            .iter()
            .take_while(|a| *a == &atoms[start])
            .count();
        mask.push_str(&atoms[start]);
        if count > 1 {
            mask.push_str(&format!("{{{count}}}"));
        }
        start += count;
    }
    Some(LiteralGate::CharacterMask(mask))
}

/// Plan an opt-in shared candidate scan followed by exact, unindexed dispatch.
///
/// Admits at most 2000 raw, printable-ASCII literals of 3..=64 bytes expressed as
/// regexes. Anchors, alternation, case folding and transformed views decline.
/// Columns must be unqualified identifiers so their names survive materialization.
/// `None` means retain the original complete sweep; no input rule is dropped.
///
/// This only generates SQL. The caller must choose staging from measured cost /
/// selectivity, enforce acquisition and aggregate worker memory/deadlines, preserve
/// a source snapshot, and cancel/join workers before dropping its uniquely owned
/// table on success, error or cancellation. Never truncate candidates to fit a
/// budget. A post-acquisition admission refusal has already paid acquisition cost.
/// Source filters are trusted SQL with the same contract as ordinary sweep APIs.
///
/// # Errors
/// Returns errors for invalid identifiers, invalid admitted regex syntax, invalid
/// query limits, or SQL/AST budgets that cannot hold the complete plan.
#[allow(clippy::too_many_arguments)]
pub fn staged_literal_sweep(
    source: &str,
    staging: &str,
    select: &[&str],
    cols: &ViewColumns,
    items: &[SweepItem],
    filter: Option<&str>,
    options: SweepOptions,
) -> Result<Option<StagedLiteralPlan>, ChError> {
    if options.max_bytes == 0 || options.max_ast_nodes == 0 {
        return Err(ChError::InvalidSweepOptions);
    }
    // Bound caller-owned text before cloning/rendering it. These are lower
    // bounds; the completed statements are checked again below.
    if source.len() > options.max_bytes
        || staging.len() > options.max_bytes
        || filter.is_some_and(|f| f.len() > options.max_bytes)
        || select.iter().fold(0usize, |n, c| n.saturating_add(c.len())) > options.max_bytes
    {
        return Err(ChError::QueryTooLarge(options.max_bytes));
    }
    let source = ident(source)?;
    let staging = ident(staging)?;
    if source == staging {
        return Err(ChError::Range);
    }
    let value = cols.materialized.get(&View::Raw).unwrap_or(&cols.raw);
    if value.len() > options.max_bytes {
        return Err(ChError::QueryTooLarge(options.max_bytes));
    }
    let value = value.clone();
    let mut projected = select
        .iter()
        .map(|c| ident(c))
        .collect::<Result<Vec<_>, _>>()?;
    if items.is_empty()
        || items.len() > 2000
        || ident(&value).is_err()
        || value.contains('.')
        || projected.iter().any(|c| c.contains('.'))
    {
        return Ok(None);
    }
    let mut literals = Vec::with_capacity(items.len());
    for item in items {
        let Some(value) = literal(item)? else {
            return Ok(None);
        };
        literals.push(value);
    }
    let Some(gate) = necessary_gate(&literals) else {
        return Ok(None);
    };
    let predicate = match &gate {
        LiteralGate::Prefix(prefix) => format!("position({value},{}) > 0", quote(prefix)),
        LiteralGate::CharacterMask(mask) => format!("match({value},{})", quote(mask)),
    };
    if !projected.contains(&value) {
        projected.push(value);
    }
    // Preserve repeated output projections in dispatch, but store each source
    // column once. Equal source rows remain separate rows (no DISTINCT).
    let mut seen = BTreeSet::new();
    projected.retain(|col| seen.insert(col.clone()));
    let mut candidate_select = format!(
        "SELECT {}\nFROM {source}\nWHERE ({predicate})",
        projected.join(", ")
    );
    let mut estimated_ast = super::sweep::expression_ast(&candidate_select)?;
    if let Some(filter) = filter {
        // Trusted source SQL is not restricted to the generated-expression
        // grammar. Match ordinary sweeps' conservative byte-based accounting.
        estimated_ast = estimated_ast
            .saturating_add(filter.len().saturating_mul(4))
            .saturating_add(16);
        candidate_select.push_str(&format!("\nAND (\n{filter}\n)"));
    }
    if candidate_select.len() > options.max_bytes {
        return Err(ChError::QueryTooLarge(options.max_bytes));
    }
    if estimated_ast > options.max_ast_nodes {
        return Err(ChError::SweepAstTooLarge(options.max_ast_nodes));
    }
    let mut queries = Vec::with_capacity(items.len().div_ceil(250));
    for batch in items.chunks(250) {
        let plan = super::indexed_dispatch::sweep_byte_dispatch_with_gate(
            &staging, select, cols, batch, None, options, None, false,
        )?;
        if plan.fallback.is_some() {
            return Ok(None);
        }
        queries.extend(plan.sweep.queries);
    }
    Ok(Some(StagedLiteralPlan {
        gate,
        candidate_select,
        queries,
    }))
}
