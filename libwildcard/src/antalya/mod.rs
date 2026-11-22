// Copyright 2026 The pg_wildcard Authors
// SPDX-License-Identifier: Apache-2.0

//! ClickHouse SQL generation.
//!
//! Every generated expression must return exactly what `wildcard-core`
//! returns for the same input. `tests/differential` checks this against an
//! embedded ClickHouse (chDB); run it after any change here.

use std::collections::BTreeMap;

use crate::core::pattern::Piece;
use thiserror::Error;
mod adaptive;
mod indexed_dispatch;
pub use indexed_dispatch::{
    ByteDispatchFallback, ByteDispatchGate, ByteDispatchPlan, ByteDispatchStrategy,
    ByteDispatchSweepPlan, byte_gram_index_ddl, sweep_byte_dispatch,
};
pub mod identities;
mod lake_filter;
pub use lake_filter::{
    LakeFilterStrategy, LakePathFilter, MAX_LAKE_FILTER_BYTES, lake_coverage_id, lake_path_filter,
    lake_path_filter_with_inventory,
};
mod prefilter;
mod storage;
mod sweep;
mod watchlist;
pub use adaptive::{CandidateDecision, PlanEstimate, choose_candidate, native_plan_estimate};
pub use storage::MergeTreeSearchLayout;
#[cfg(test)]
use sweep::sweep_sql;
pub use sweep::{
    DEFAULT_SWEEP_MAX_AST_NODES, SweepOptions, SweepPlan, SweepStrategy, sweep_queries,
    sweep_queries_with_options,
};
pub use watchlist::{WatchlistFallback, WatchlistPlan, WatchlistStrategy, watchlist_plan};
pub mod ranges;
pub use prefilter::{
    GramFileCounts, candidate_predicate, watchlist_needles, watchlist_needles_with_frequencies,
};

use crate::core::verify::{self, Kind};
use crate::core::{View, re2};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChError {
    #[error(transparent)]
    PatternTooLong(#[from] crate::core::pattern::PatternTooLong),
    #[error("invalid or oversized candidate range")]
    Range,
    #[error("a sweep query cannot fit within {0} bytes")]
    QueryTooLarge(usize),
    #[error("a sweep query cannot fit within {0} estimated AST nodes")]
    SweepAstTooLarge(usize),
    #[error("sweep limits must be positive")]
    InvalidSweepOptions,
    #[error("watchlist views must be nonempty and contain no duplicates")]
    InvalidWatchlistViews,
    #[error("sweep SQL fragment cannot be conservatively budgeted")]
    UnbudgetableSweepSql,
    #[error("invalid identifier '{0}'")]
    Identifier(String),
    #[error(transparent)]
    Re2(#[from] re2::Re2Error),
}

/// ClickHouse string literal. Escapes `\`, `'` and control characters.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02X}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Validate a (possibly qualified) identifier: `db.table`, `column`.
pub fn ident(name: &str) -> Result<String, ChError> {
    let ok = !name.is_empty()
        && name.split('.').all(|p| {
            let mut ch = p.chars();
            ch.next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && ch.all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
    if ok {
        Ok(name.to_string())
    } else {
        Err(ChError::Identifier(name.to_string()))
    }
}

/// Bind `expr` to `name` inside `body` without repeating it:
/// `arrayElement(arrayMap(name -> body, [expr]), 1)`.
fn bind(name: &str, expr: &str, body: &str) -> String {
    format!("arrayElement(arrayMap({name} -> {body}, [{expr}]), 1)")
}

/// ClickHouse expression computing `view` of the column expression `col`,
/// matching `crate::core::views` byte for byte.
pub fn view_expr(col: &str, view: View) -> String {
    match view {
        View::Raw => col.to_string(),
        View::Lower => format!("lower({col})"),
        View::Decoded => format!("lower(decodeURLComponent({col}))"),
        View::WinPath => format!("lower(replaceAll({col}, {}, '/'))", quote("\\")),
        View::RevDomain => {
            let reversed = "if(startsWith(h4, '['), h4, arrayStringConcat(arrayReverse(splitByChar('.', h4)), '.'))";
            let h4 = bind(
                "h4",
                "if(endsWith(h3, '.'), substring(h3, 1, length(h3) - 1), h3)",
                reversed,
            );
            let h3 = bind("h3", "lower(h2)", &h4);
            let port = "if(length(splitByChar(':', h1)) > 1 AND match(arrayElement(splitByChar(':', h1), -1), '^[0-9]*$'), \
                        arrayStringConcat(arraySlice(splitByChar(':', h1), 1, length(splitByChar(':', h1)) - 1), ':'), h1)";
            let h2expr = format!(
                "if(startsWith(h1, '['), if(position(h1, ']') > 0, substring(h1, 1, position(h1, ']')), h1), {port})"
            );
            let h2 = bind("h2", &h2expr, &h3);
            let h1 = bind("h1", "arrayElement(splitByChar('@', auth), -1)", &h2);
            let auth = bind(
                "auth",
                "if(multiSearchFirstPosition(rest, ['/', '?', '#']) > 0, substring(rest, 1, multiSearchFirstPosition(rest, ['/', '?', '#']) - 1), rest)",
                &h1,
            );
            bind(
                "rest",
                &format!(
                    "if(position({col}, '://') > 0, substring({col}, position({col}, '://') + 3), {col})"
                ),
                &auth,
            )
        }
    }
}

/// Columns already holding materialized views of a source column. When a view
/// has a materialized column, it is used directly so text indexes apply.
#[derive(Clone, Debug, Default)]
pub struct ViewColumns {
    pub raw: String,
    pub materialized: BTreeMap<View, String>,
}

impl ViewColumns {
    pub fn new(raw: &str) -> Result<Self, ChError> {
        Ok(Self {
            raw: ident(raw)?,
            materialized: BTreeMap::new(),
        })
    }

    /// Conventional names `<col>_<view>` for every non-raw view.
    pub fn conventional(raw: &str) -> Result<Self, ChError> {
        let mut v = Self::new(raw)?;
        for view in View::ALL.into_iter().filter(|v| *v != View::Raw) {
            v.materialized
                .insert(view, format!("{raw}_{}", view.name()));
        }
        Ok(v)
    }

    pub fn expr(&self, view: View) -> String {
        self.materialized
            .get(&view)
            .cloned()
            .unwrap_or_else(|| view_expr(&self.raw, view))
    }
}

/// ClickHouse `LIKE` pattern for pieces (canonical escaping).
pub fn like_pattern(pieces: &[Piece]) -> String {
    let mut s = String::new();
    for p in pieces {
        match p {
            Piece::Lit(v) => {
                for c in String::from_utf8_lossy(v).chars() {
                    if matches!(c, '\\' | '%' | '_') {
                        s.push('\\');
                    }
                    s.push(c);
                }
            }
            Piece::AnyOne => s.push('_'),
            Piece::AnyMany => s.push('%'),
        }
    }
    s
}

/// Boolean ClickHouse expression: `value_expr` matches `pattern`.
pub fn predicate(
    value_expr: &str,
    kind: Kind,
    pattern: &str,
    view: View,
) -> Result<String, ChError> {
    Ok(match kind {
        Kind::Wildcard | Kind::Like => {
            let pieces = verify::pieces_for_checked(kind, pattern.as_bytes(), view)?;
            format!("like({value_expr}, {})", quote(&like_pattern(&pieces)))
        }
        Kind::Regex { case_insensitive } => {
            re2::to_rust(pattern)?; // validates RE2 portability
            let p = if case_insensitive {
                format!("(?i){pattern}")
            } else {
                pattern.to_string()
            };
            format!("match({value_expr}, {})", quote(&p))
        }
    })
}

/// Predicate on a column with view resolution.
pub fn column_predicate(
    cols: &ViewColumns,
    kind: Kind,
    pattern: &str,
    view: View,
) -> Result<String, ChError> {
    predicate(&cols.expr(view), kind, pattern, view)
}

/// One indicator in a sweep or watchlist: RE2 regex searched in a view.
#[derive(Clone, Debug)]
pub struct SweepItem {
    pub id: i64,
    pub view: View,
    pub regex: String,
}

/// Historical batching limit, retained for source compatibility only.
/// Sweeps now use [`SweepOptions`] byte and AST budgets; this is not an active cap.
#[deprecated(note = "use SweepOptions byte and AST budgets; this is only the legacy limit")]
pub const MAX_SWEEP_ITEMS: usize = 64;

/// DDL: materialized view columns plus n-gram text indexes.
pub fn view_columns_ddl(table: &str, col: &str, views: &[View]) -> Result<Vec<String>, ChError> {
    let (table, col) = (ident(table)?, ident(col)?);
    let mut out = Vec::new();
    for &v in views {
        let target = if v == View::Raw {
            col.clone()
        } else {
            let name = format!("{col}_{}", v.name());
            out.push(format!(
                "ALTER TABLE {table} ADD COLUMN IF NOT EXISTS {name} String MATERIALIZED {}",
                view_expr(&col, v)
            ));
            name
        };
        out.push(format!(
            "ALTER TABLE {table} ADD INDEX IF NOT EXISTS {target}_ngrams {target} TYPE text(tokenizer = 'ngrams') GRANULARITY 1"
        ));
        out.push(format!(
            "ALTER TABLE {table} MATERIALIZE INDEX {target}_ngrams"
        ));
    }
    Ok(out)
}

/// DDL for the watchlist table in ClickHouse (filled from Postgres).
pub fn watchlist_table_ddl(watch: &str) -> Result<String, ChError> {
    Ok(format!(
        "CREATE TABLE IF NOT EXISTS {} (id Int64, view LowCardinality(String), regex String, needles Array(String) DEFAULT [], needles_regex String DEFAULT '', \
         needles_view String DEFAULT '', active UInt8 DEFAULT 1, \
         updated_at DateTime64(3) DEFAULT now64(3)) ENGINE = ReplacingMergeTree(updated_at) ORDER BY id",
        ident(watch)?
    ))
}

/// Insert validated derived metadata together with its regex/view binding.
/// Callers updating regex/view alone safely fall back to exact matching. Direct
/// writers must use this helper or leave needles empty; bindings detect stale
/// metadata, not maliciously fabricated matching bindings.
pub fn watchlist_insert_sql(watch: &str, item: &SweepItem) -> Result<String, ChError> {
    watchlist_insert_sql_with_frequencies(watch, item, None)
}

/// Insert required needles selected using optional observed file counts.
/// Statistics affect selection only; regex/view bindings protect stale metadata.
pub fn watchlist_insert_sql_with_frequencies(
    watch: &str,
    item: &SweepItem,
    frequencies: Option<&GramFileCounts>,
) -> Result<String, ChError> {
    let needles = watchlist_needles_with_frequencies(&item.regex, item.view, frequencies)?;
    Ok(format!(
        "INSERT INTO {} (id, view, regex, needles, needles_regex, needles_view) VALUES ({}, {}, {}, [{}], {}, {})",
        ident(watch)?,
        item.id,
        quote(item.view.name()),
        quote(&item.regex),
        needles
            .iter()
            .map(|s| quote(s))
            .collect::<Vec<_>>()
            .join(", "),
        quote(&item.regex),
        quote(item.view.name())
    ))
}

/// Materialized view matching every inserted row against the active
/// watchlist. The indicator list is read by scalar subqueries for each insert
/// block, so the view never needs to be recreated when indicators change.
pub fn watchlist_mv(
    name: &str,
    source: &str,
    hits: &str,
    watch: &str,
    select: &[&str],
    cols: &ViewColumns,
    views: &[View],
) -> Result<String, ChError> {
    let (name, source, hits, watch) = (ident(name)?, ident(source)?, ident(hits)?, ident(watch)?);
    let select: Vec<String> = select.iter().map(|c| ident(c)).collect::<Result<_, _>>()?;
    let mut with = Vec::new();
    let mut arrays = Vec::new();
    for &view in views {
        let n = view.name();
        // Three scalar slots avoid row × indicator × needle array expansion.
        // Missing/stale slots become empty strings; position(value, '') = 1.
        let slots = (1..=3).map(|slot| format!(
            "groupArray(if(needles_regex = regex AND needles_view = view, arrayElement(needles, {slot}), ''))"
        )).collect::<Vec<_>>().join(", ");
        with.push(format!(
            "(SELECT (groupArray(id), groupArray(regex), {slots}) FROM (SELECT id, regex, view, needles, needles_regex, needles_view FROM {watch} FINAL WHERE active = 1 AND view = '{n}' ORDER BY id)) AS wc_w_{n}"
        ));
        let value = cols.expr(view);
        let mut candidates = format!("arrayEnumerate(tupleElement(wc_w_{n}, 2))");
        for slot in 3..=5 {
            candidates = format!(
                "arrayFilter(i -> position({value}, arrayElement(tupleElement(wc_w_{n}, {slot}), i)) > 0, {candidates})"
            );
        }
        arrays.push(format!(
            "arrayMap(i -> (arrayElement(tupleElement(wc_w_{n}, 1), i), '{n}'), arrayFilter(i -> match({value}, arrayElement(tupleElement(wc_w_{n}, 2), i)), {candidates}))"
        ));
    }
    Ok(format!(
        "CREATE MATERIALIZED VIEW IF NOT EXISTS {name} TO {hits} AS\nWITH {}\nSELECT {}{}tupleElement(wc_hit, 1) AS indicator_id, tupleElement(wc_hit, 2) AS view\nFROM {source}\nARRAY JOIN arrayConcat({}) AS wc_hit",
        with.join(", "),
        select.join(", "),
        if select.is_empty() { "" } else { ", " },
        arrays.join(",\n  ")
    ))
}

/// Query over an Iceberg table (Antalya), restricted to candidate data files
/// from the Puffin n-gram index. `None` when there are no candidates.
/// The caller must establish that this candidate set covers the queried snapshot.
/// Prefer [`lake_query_with_coverage`] when the table may contain newer files.
pub fn lake_query(
    table: &str,
    select: &str,
    predicate: &str,
    files: Option<&[String]>,
    extra: Option<&str>,
) -> Result<Option<String>, ChError> {
    let table = ident(table)?;
    let mut conds = Vec::new();
    if let Some(files) = files {
        if files.is_empty() {
            return Ok(None);
        }
        conds.push(format!(
            "_path IN ({})",
            files
                .iter()
                .map(|f| quote(f))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(e) = extra {
        conds.push(format!("({e})"));
    }
    conds.push(predicate.to_string());
    Ok(Some(format!(
        "SELECT {select} FROM {table} WHERE {}",
        conds.join(" AND ")
    )))
}

/// Prune indexed immutable files while retaining every unknown current file.
///
/// `indexed` is the exact inventory of the supplied index, not an assertion that
/// it describes the entire current table. Candidates must be a subset of it.
/// Iceberg file paths must be immutable; this does not authenticate an index.
/// If the escaped path guard would exceed 64 KiB, query all files while keeping
/// the exact predicate and `extra` filter.
pub fn lake_query_with_coverage(
    table: &str,
    select: &str,
    predicate: &str,
    indexed: &[String],
    candidates: &[String],
    extra: Option<&str>,
) -> Result<String, ChError> {
    lake_query_with_inventory(table, select, predicate, indexed, candidates, extra, None)
        .map(|(sql, _)| sql)
}

/// Coverage-safe lake query with an optional content-addressed server inventory.
/// Returns the selected strategy, including a visible 64 KiB fallback reason.
/// See [`lake_path_filter_with_inventory`] for the loader's trust contract.
pub fn lake_query_with_inventory(
    table: &str,
    select: &str,
    predicate: &str,
    indexed: &[String],
    candidates: &[String],
    extra: Option<&str>,
    known_table: Option<&str>,
) -> Result<(String, LakeFilterStrategy), ChError> {
    ident(table)?;
    let filter = lake_path_filter_with_inventory(indexed, candidates, known_table)?;
    let guard = match (filter.condition, extra) {
        (Some(guard), Some(extra)) => Some(format!("({guard}) AND ({extra})")),
        (Some(guard), None) => Some(guard),
        (None, Some(extra)) => Some(extra.to_string()),
        (None, None) => None,
    };
    let sql =
        lake_query(table, select, predicate, None, guard.as_deref())?.ok_or(ChError::Range)?;
    Ok((sql, filter.strategy))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_source_patterns_are_rejected_before_sql_generation() {
        let pattern = "%".repeat(crate::core::pattern::MAX_PATTERN_BYTES + 1);
        for kind in [
            Kind::Like,
            Kind::Wildcard,
            Kind::Regex {
                case_insensitive: false,
            },
        ] {
            let error = predicate("message", kind, &pattern, View::Raw).unwrap_err();
            assert!(error.to_string().contains("pattern exceeds"), "{error}");
        }
    }

    #[test]
    fn quoting() {
        assert_eq!(quote(r"a'b\c"), r"'a\'b\\c'");
        assert_eq!(quote("x\ny\u{1}"), r"'x\ny\x01'");
        assert!(ident("db.events").is_ok());
        assert!(ident("x; DROP TABLE y").is_err());
        assert!(ident("").is_err());
    }

    #[test]
    fn predicates() {
        let p = predicate("url", Kind::Wildcard, "*/wp_admin/*.PHP", View::Lower).unwrap();
        assert_eq!(p, r"like(url, '%/wp\\_admin/%.php')");
        let p = predicate(
            "u",
            Kind::Regex {
                case_insensitive: true,
            },
            r"cmd\.exe",
            View::Raw,
        )
        .unwrap();
        assert_eq!(p, r"match(u, '(?i)cmd\\.exe')");
        assert!(
            predicate(
                "u",
                Kind::Regex {
                    case_insensitive: false
                },
                "[a&&b]",
                View::Raw
            )
            .is_err()
        );
    }

    #[test]
    fn watchlist_uses_three_bound_scalar_needles() {
        let item = SweepItem {
            id: 1,
            view: View::Raw,
            regex: "abc".into(),
        };
        let insert = watchlist_insert_sql("watch", &item).unwrap();
        assert!(insert.contains("needles_regex, needles_view"));
        assert!(insert.contains("['abc'], 'abc', 'raw'"));
        let ddl = watchlist_table_ddl("watch").unwrap();
        assert!(ddl.contains("needles Array(String) DEFAULT []"));
        let mv = watchlist_mv(
            "mv",
            "logs",
            "hits",
            "watch",
            &["id"],
            &ViewColumns::new("v").unwrap(),
            &[View::Raw],
        )
        .unwrap();
        assert!(mv.contains("needles_regex = regex AND needles_view = view"));
        assert!(mv.contains("position(v, arrayElement(tupleElement(wc_w_raw, 3), i)) > 0"));
        for slot in 1..=3 {
            assert!(mv.contains(&format!("arrayElement(needles, {slot})")));
            assert!(mv.contains(&format!("tupleElement(wc_w_raw, {})", slot + 2)));
        }
        assert!(!mv.contains("arrayAll"));
        assert_eq!(mv.matches("arrayFilter(").count(), 4);
        assert_eq!(mv.matches("arrayEnumerate(").count(), 1);
        assert!(mv.contains("arrayFilter(i -> match(v, arrayElement(tupleElement(wc_w_raw, 2), i)), arrayFilter(i -> position"));
    }

    #[test]
    fn sweep_widens_when_only_the_exact_expression_fits() {
        let item = SweepItem {
            id: 1,
            view: View::Raw,
            regex: "abc".into(),
        };
        let exact = sweep_sql(
            "logs",
            &["id".into()],
            &["(toInt64(1), 'raw', (match(v, 'abc')))".into()],
            None,
        );
        let queries = sweep_queries(
            "logs",
            &["id"],
            &ViewColumns::new("v").unwrap(),
            &[item],
            None,
            exact.len(),
        )
        .unwrap();
        assert_eq!(queries, vec![exact]);
    }

    #[test]
    fn sweep_thousand_small_indicators_use_at_most_two_scans() {
        let items: Vec<_> = (0..1000)
            .map(|id| SweepItem {
                id,
                view: View::Raw,
                regex: format!("evil{id:04}\\.example"),
            })
            .collect();
        let queries = sweep_queries(
            "bench",
            &["id"],
            &ViewColumns::new("v").unwrap(),
            &items,
            None,
            200_000,
        )
        .unwrap();
        assert!(queries.len() <= 2, "{} source scans", queries.len());
    }

    #[test]
    fn sweep_batches() {
        let cols = ViewColumns::new("url").unwrap();
        let items: Vec<SweepItem> = (0..1000)
            .map(|i| SweepItem {
                id: i,
                view: View::Lower,
                regex: format!("evil{i}\\.com"),
            })
            .collect();
        let qs = sweep_queries(
            "logs",
            &["id"],
            &cols,
            &items,
            Some("ts > now() - INTERVAL 7 DAY"),
            8 << 10,
        )
        .unwrap();
        assert!(qs.len() > 1);
        assert!(qs.iter().all(|q| q.len() < 8 << 10));
    }

    #[test]
    fn sweep_sql_budget_counts_escaping_and_fixed_sql() {
        let cols = ViewColumns::new("v").unwrap();
        let items: Vec<_> = View::ALL
            .iter()
            .cycle()
            .take(12)
            .enumerate()
            .map(|(i, view)| SweepItem {
                id: i as i64,
                view: *view,
                regex: "\0".repeat(500),
            })
            .collect();
        let qs = sweep_queries("logs", &["id"], &cols, &items, None, 7000).unwrap();
        assert!(
            qs.iter().all(|q| q.len() <= 7000),
            "{:?}",
            qs.iter().map(String::len).collect::<Vec<_>>()
        );
        let one = sweep_queries("logs", &["id"], &cols, &items[..1], None, 10000)
            .unwrap()
            .remove(0);
        assert_eq!(
            sweep_queries("logs", &["id"], &cols, &items[..1], None, one.len()).unwrap(),
            vec![one.clone()]
        );
        assert!(sweep_queries("logs", &["id"], &cols, &items[..1], None, one.len() - 1).is_err());
        assert!(
            sweep_queries(
                "logs",
                &["id"],
                &cols,
                &items[..1],
                Some(&"1".repeat(8000)),
                7000
            )
            .is_err()
        );
        assert!(sweep_queries("logs", &["id"], &cols, &items[..1], None, 0).is_err());
        assert!(
            sweep_queries("logs", &[], &cols, &[], None, 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn lake() {
        assert_eq!(
            lake_query("lake.http", "*", "1", Some(&[]), None).unwrap(),
            None
        );
        let q = lake_query(
            "lake.http",
            "url",
            "like(url, '%x%')",
            Some(&["s3://b/f1.parquet".into()]),
            None,
        )
        .unwrap()
        .unwrap();
        assert!(q.contains("_path IN ('s3://b/f1.parquet')"));
    }
    #[test]
    fn native_candidates_preserve_alternation_and_escape_literals() {
        let candidate = candidate_predicate(
            "v",
            Kind::Regex {
                case_insensitive: false,
            },
            "powershell|cmd",
            View::Raw,
        )
        .unwrap()
        .unwrap();
        assert!(candidate.contains(" OR "));
        assert!(candidate.contains("%cmd%"));
        assert!(
            candidate_predicate(
                "v",
                Kind::Regex {
                    case_insensitive: false
                },
                "powershell|.*",
                View::Raw
            )
            .unwrap()
            .is_none()
        );
        let escaped = candidate_predicate("v", Kind::Like, r"%a\%b%", View::Raw)
            .unwrap()
            .unwrap();
        assert!(escaped.contains(r"a\\%b"));
        assert!(
            candidate_predicate("v", Kind::Wildcard, "*é*", View::Raw)
                .unwrap()
                .is_none()
        );
    }
}
