// Copyright 2026 The pg_wildcard Authors
// SPDX-License-Identifier: Apache-2.0

use super::{ChError, ViewColumns, ident, quote, watchlist_mv};
use crate::core::View;

const MAX_VALUE_BYTES: usize = 256;

/// Execution strategy for newly inserted rows and the current watchlist.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WatchlistStrategy {
    #[default]
    StagedNeedles,
    /// Join byte trigrams to required rule anchors, then recheck exactly.
    /// Currently supports only an explicitly configured single raw view.
    RuleAnchorJoin,
}

impl WatchlistStrategy {
    pub fn name(self) -> &'static str {
        match self {
            Self::StagedNeedles => "staged_needles",
            Self::RuleAnchorJoin => "rule_anchor_join",
        }
    }
}

/// Why a requested strategy used the existing staged implementation instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchlistFallback {
    RequiresSingleRawView,
    InternalAliasConflict,
}

impl WatchlistFallback {
    pub fn name(self) -> &'static str {
        match self {
            Self::RequiresSingleRawView => "requires_single_raw_view",
            Self::InternalAliasConflict => "internal_alias_conflict",
        }
    }
}

/// Generated DDL and the strategy actually selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchlistPlan {
    pub sql: String,
    pub strategy: WatchlistStrategy,
    pub fallback: Option<WatchlistFallback>,
    /// Values exceeding this byte length bypass gram extraction.
    pub max_value_bytes: Option<usize>,
    /// Bound per source row, not a bound on matching rules or total join memory.
    pub max_source_keys: Option<usize>,
}

/// Build an opt-in watchlist strategy without changing [`watchlist_mv`].
///
/// The join reads active rules with `FINAL` for each inserted block. It preserves
/// raw values, duplicate source rows and every matching rule ID. Missing or stale
/// needle bindings and long values retain exact checks. Direct watchlist writers
/// must supply proven required needles or leave them empty, as with the staged
/// strategy. Bindings detect stale metadata, not deliberately forged metadata.
///
/// The 256-byte extraction cap bounds source keys to 255 per row. Rules without
/// useful anchors can still produce a row-by-rule join; engine memory/time limits
/// remain necessary. Other view configurations fall back to staged needles.
#[allow(clippy::too_many_arguments)]
pub fn watchlist_plan(
    name: &str,
    source: &str,
    hits: &str,
    watch: &str,
    select: &[&str],
    cols: &ViewColumns,
    views: &[View],
    strategy: WatchlistStrategy,
) -> Result<WatchlistPlan, ChError> {
    if views.is_empty()
        || views.len() > View::ALL.len()
        || views
            .iter()
            .enumerate()
            .any(|(i, v)| views[..i].contains(v))
    {
        return Err(ChError::InvalidWatchlistViews);
    }
    // ViewColumns fields are public; do not rely on its constructor being used.
    for name in [name, source, hits, watch, &cols.raw]
        .into_iter()
        .chain(select.iter().copied())
        .chain(
            views
                .iter()
                .filter_map(|v| cols.materialized.get(v).map(String::as_str)),
        )
    {
        ident(name)?;
    }
    let prefix = (0..16).map(|i| format!("_wc{i}_")).find(|prefix| {
        [name, source, hits, watch, &cols.raw]
            .into_iter()
            .chain(select.iter().copied())
            .chain(cols.materialized.values().map(String::as_str))
            .all(|name| !name.contains(prefix))
    });
    let fallback = match strategy {
        WatchlistStrategy::StagedNeedles => None,
        WatchlistStrategy::RuleAnchorJoin if views != [View::Raw] => {
            Some(WatchlistFallback::RequiresSingleRawView)
        }
        WatchlistStrategy::RuleAnchorJoin if prefix.is_none() => {
            Some(WatchlistFallback::InternalAliasConflict)
        }
        WatchlistStrategy::RuleAnchorJoin => {
            // The preceding branch established that an unused prefix exists.
            if let Some(prefix) = prefix {
                return Ok(WatchlistPlan {
                    sql: rule_join(
                        name,
                        source,
                        hits,
                        watch,
                        select,
                        &cols.expr(View::Raw),
                        &prefix,
                    ),
                    strategy,
                    fallback: None,
                    max_value_bytes: Some(MAX_VALUE_BYTES),
                    max_source_keys: Some(MAX_VALUE_BYTES - 1),
                });
            }
            Some(WatchlistFallback::InternalAliasConflict)
        }
    };
    Ok(WatchlistPlan {
        sql: watchlist_mv(name, source, hits, watch, select, cols, views)?,
        strategy: WatchlistStrategy::StagedNeedles,
        fallback,
        max_value_bytes: None,
        max_source_keys: None,
    })
}

// All caller-provided identifiers have been validated by watchlist_plan.
fn rule_join(
    name: &str,
    source: &str,
    hits: &str,
    watch: &str,
    select: &[&str],
    value: &str,
    p: &str,
) -> String {
    let fallback = quote("_wc_no_anchor");
    let overflow = quote("_wc_long_value");
    let raw = quote(View::Raw.name());
    let needles = "if(needles_regex = regex AND needles_view = view, \
         arraySlice(needles, 1, 3), CAST([], 'Array(String)'))";
    let gram = format!("arrayElement({p}needles, 1)");
    let anchor = format!(
        "if(length({gram}) = 3 AND arrayAll({p}i -> \
         ascii(substring({gram}, {p}i, 1)) BETWEEN 32 AND 126, [1, 2, 3]), \
         {gram}, {fallback})"
    );
    // Bound range itself: correctness must not depend on IF short-circuiting.
    let grams = format!(
        "arrayDistinct(arrayMap({p}i -> substring({p}value, {p}i + 1, 3), \
         range(toUInt64(greatest(toInt64(least(length({p}value), {MAX_VALUE_BYTES})) - 2, 0)))))"
    );
    let keys = format!(
        "if(length({p}value) > {MAX_VALUE_BYTES}, [{overflow}], \
         arrayConcat({grams}, [{fallback}]))"
    );
    let projection = if select.is_empty() {
        String::new()
    } else {
        format!("{}, ", select.join(", "))
    };
    format!(
        "CREATE MATERIALIZED VIEW IF NOT EXISTS {name} TO {hits} AS\n\
         SELECT {p}src.* EXCEPT ({p}value, {p}key), {p}rule.id AS indicator_id, {raw} AS view\n\
         FROM (\n  SELECT {projection}{value} AS {p}value, arrayJoin({keys}) AS {p}key\n  FROM {source}\n) AS {p}src\n\
         ALL INNER JOIN (\n  SELECT id, regex, {p}needles, \
         arrayJoin([{anchor}, {overflow}]) AS {p}key\n  FROM (\n    \
         SELECT id, regex, {needles} AS {p}needles\n    \
         FROM {watch} FINAL WHERE active = 1 AND view = {raw}\n  )\n) AS {p}rule \
         ON {p}src.{p}key = {p}rule.{p}key\n\
         WHERE arrayAll({p}n -> position({p}src.{p}value, {p}n) > 0, {p}rule.{p}needles)\n  \
         AND match({p}src.{p}value, {p}rule.regex)"
    )
}
