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
pub mod identities;
mod lake_filter;
pub use lake_filter::lake_path_filter;
mod prefilter;
mod storage;
pub use adaptive::{CandidateDecision, PlanEstimate, choose_candidate, native_plan_estimate};
pub use storage::MergeTreeSearchLayout;
pub mod ranges;
pub use prefilter::candidate_predicate;

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

fn hit_array(view_sql: &str, view: View, ids: &str, pats: &str) -> String {
    // Hyperscan is not a sound candidate filter for RE2: Unicode versions and
    // invalid UTF-8 handling differ. Enumerate all indicators for exact checks,
    // including dynamic watchlist patterns, until a safe subset is established.
    format!(
        "arrayMap(i -> (arrayElement({ids}, i), '{v}'), arrayFilter(i -> match({view_sql}, arrayElement({pats}, i)), arrayEnumerate({pats})))",
        v = view.name()
    )
}

/// IOC sweep queries, batched so each stays under `max_bytes` of SQL
/// (ClickHouse `max_query_size` defaults to 256 KiB). Each returns
/// `(select_cols..., indicator_id, view)`. Every indicator is checked with
/// `match()`; an item that cannot fit in one query returns `QueryTooLarge`.
pub fn sweep_queries(
    source: &str,
    select: &[&str],
    cols: &ViewColumns,
    items: &[SweepItem],
    filter: Option<&str>,
    max_bytes: usize,
) -> Result<Vec<String>, ChError> {
    let source = ident(source)?;
    let select: Vec<String> = select.iter().map(|c| ident(c)).collect::<Result<_, _>>()?;
    for it in items {
        re2::to_rust(&it.regex)?;
    }
    // Count the rendered SQL, including quoting, identifiers, view expressions
    // and the caller's filter. Delimiters are the only non-payload growth when
    // appending to an existing view. Work stays linear in emitted SQL size.
    let mut batch = BTreeMap::new();
    let base_bytes = sweep_sql(&source, &select, cols, &batch, filter).len();
    let mut size = base_bytes;
    let mut view_overheads = BTreeMap::new();
    let mut out = Vec::new();
    for item in items {
        let overhead = *view_overheads.entry(item.view).or_insert_with(|| {
            let empty_view = BTreeMap::from([(item.view, Vec::new())]);
            sweep_sql(&source, &select, cols, &empty_view, filter).len() - base_bytes
        });
        let payload = quote(&item.regex).len() + item.id.to_string().len();
        let cost = if batch.contains_key(&item.view) {
            payload + 4 // comma and space in both the ID and pattern arrays
        } else {
            overhead + payload + if batch.is_empty() { 0 } else { 6 }
        };
        if size.checked_add(cost).is_none_or(|n| n > max_bytes) {
            if !batch.is_empty() {
                out.push(sweep_sql(&source, &select, cols, &batch, filter));
                batch.clear();
            }
            size = base_bytes
                .checked_add(overhead)
                .and_then(|n| n.checked_add(payload))
                .filter(|&n| n <= max_bytes)
                .ok_or(ChError::QueryTooLarge(max_bytes))?;
        } else {
            size += cost;
        }
        batch.entry(item.view).or_insert_with(Vec::new).push(item);
    }
    if !batch.is_empty() {
        out.push(sweep_sql(&source, &select, cols, &batch, filter));
    }
    debug_assert!(out.iter().all(|sql| sql.len() <= max_bytes));
    Ok(out)
}

fn sweep_sql(
    source: &str,
    select: &[String],
    cols: &ViewColumns,
    by_view: &BTreeMap<View, Vec<&SweepItem>>,
    filter: Option<&str>,
) -> String {
    let mut with = Vec::new();
    let mut arrays = Vec::new();
    for (view, its) in by_view {
        let n = view.name();
        let ids: Vec<String> = its.iter().map(|i| i.id.to_string()).collect();
        let pats: Vec<String> = its.iter().map(|i| quote(&i.regex)).collect();
        with.push(format!("[{}] AS wc_ids_{n}", ids.join(", ")));
        with.push(format!("[{}] AS wc_pats_{n}", pats.join(", ")));
        arrays.push(hit_array(
            &cols.expr(*view),
            *view,
            &format!("wc_ids_{n}"),
            &format!("wc_pats_{n}"),
        ));
    }
    let mut sql = format!(
        "WITH {}\nSELECT {}{}tupleElement(wc_hit, 1) AS indicator_id, tupleElement(wc_hit, 2) AS view\nFROM {source}\nARRAY JOIN arrayConcat({}) AS wc_hit",
        with.join(", "),
        select.join(", "),
        if select.is_empty() { "" } else { ", " },
        arrays.join(",\n  ")
    );
    if let Some(f) = filter {
        sql.push_str(&format!("\nWHERE {f}"));
    }
    sql
}

/// DDL for the watchlist table in ClickHouse (filled from Postgres).
pub fn watchlist_table_ddl(watch: &str) -> Result<String, ChError> {
    Ok(format!(
        "CREATE TABLE IF NOT EXISTS {} (id Int64, view LowCardinality(String), regex String, active UInt8 DEFAULT 1, \
         updated_at DateTime64(3) DEFAULT now64(3)) ENGINE = ReplacingMergeTree(updated_at) ORDER BY id",
        ident(watch)?
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
        with.push(format!(
            "(SELECT (groupArray(id), groupArray(regex)) FROM (SELECT id, regex FROM {watch} FINAL WHERE active = 1 AND view = '{n}' ORDER BY id)) AS wc_w_{n}"
        ));
        arrays.push(hit_array(
            &cols.expr(view),
            view,
            &format!("tupleElement(wc_w_{n}, 1)"),
            &format!("tupleElement(wc_w_{n}, 2)"),
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
    ident(table)?;
    {
        let known: std::collections::BTreeSet<_> = indexed.iter().collect();
        if candidates.iter().any(|path| !known.contains(path)) {
            return Err(ChError::Range);
        }
    }
    let Some(guard) = lake_path_filter(indexed, candidates) else {
        return lake_query(table, select, predicate, None, extra)?.ok_or(ChError::Range);
    };
    let guard = match extra {
        Some(filter) => format!("({guard}) AND ({filter})"),
        None => guard,
    };
    lake_query(table, select, predicate, None, Some(&guard))?.ok_or(ChError::Range)
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
