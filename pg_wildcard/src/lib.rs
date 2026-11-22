// Copyright 2026 The pg_wildcard Authors
// SPDX-License-Identifier: Apache-2.0

//! pg_wildcard v2: the Postgres control plane.
//!
//! Postgres holds indicators, watchlists and cases; ClickHouse/Antalya hold
//! the telemetry. This extension
//! - evaluates patterns locally with ClickHouse semantics (`wc_match`, ...),
//! - normalises indicators (`wc_indicator`, `wc_refang`),
//! - generates ClickHouse SQL (`wc_ch_*`) to run through pg_clickhouse,
//! - prunes Iceberg data files with the Puffin n-gram index loaded into
//!   `wc_lake_*` tables (`wc_lake_candidates`, `wc_lake_sql`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use pgrx::prelude::*;
use wildcard_ch::{SweepItem, ViewColumns};
use wildcard_core::verify::Kind;
use wildcard_core::views::{self, View};
use wildcard_core::{Matcher, indicator, re2};

mod fdw;
mod matcher_cache;
mod matcher_config;
mod support;

::pgrx::pg_module_magic!(name, version);

#[pg_guard]
pub extern "C-unwind" fn _PG_init() {
    matcher_config::init();
}

thread_local! {
    /// Per-backend cache of compiled patterns (a query reuses one pattern for many rows).
    static CACHE: RefCell<matcher_cache::MatcherCache<(Kind, View), Matcher>> = RefCell::default();
}

fn view_arg(name: &str) -> View {
    name.parse::<View>()
        .unwrap_or_else(|e| error!("pg_wildcard: {e}"))
}

fn kind_arg(name: &str) -> Kind {
    name.parse::<Kind>()
        .unwrap_or_else(|e| error!("pg_wildcard: {e}"))
}

fn views_arg(list: &str) -> Vec<View> {
    list.split(',').map(|s| view_arg(s.trim())).collect()
}

fn matcher(pattern: &str, kind: Kind, view: View) -> Rc<Matcher> {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        matcher_config::configure(&mut c);
        c.get_or_compile(
            pattern,
            (kind, view),
            || Matcher::new(kind, pattern, view),
            Matcher::cache_charge,
        )
        .unwrap_or_else(|e| error!("pg_wildcard: {e}"))
    })
}

fn cols_arg(col: &str, materialized: bool) -> ViewColumns {
    let r = if materialized {
        ViewColumns::conventional(col)
    } else {
        ViewColumns::new(col)
    };
    r.unwrap_or_else(|e| error!("pg_wildcard: {e}"))
}

fn ch<T>(r: Result<T, wildcard_ch::ChError>) -> T {
    r.unwrap_or_else(|e| error!("pg_wildcard: {e}"))
}

// ---------------------------------------------------------------- matching

#[pg_extern(immutable, parallel_safe)]
fn wildcard_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Elasticsearch-style wildcard (`*`, `?`) on the whole value, ClickHouse semantics.
#[pg_extern(immutable, parallel_safe, strict)]
fn wc_match(value: &str, pattern: &str, view: default!(&str, "'raw'")) -> bool {
    matcher(pattern, Kind::Wildcard, view_arg(view)).is_match(value.as_bytes())
}

/// SQL LIKE on a view, ClickHouse semantics.
#[pg_extern(immutable, parallel_safe, strict)]
fn wc_like(value: &str, pattern: &str, view: default!(&str, "'raw'")) -> bool {
    matcher(pattern, Kind::Like, view_arg(view)).is_match(value.as_bytes())
}

/// RE2 regex search (as ClickHouse `match`), linear time.
#[pg_extern(immutable, parallel_safe, strict)]
fn wc_regex(
    value: &str,
    pattern: &str,
    case_insensitive: default!(bool, false),
    view: default!(&str, "'raw'"),
) -> bool {
    matcher(pattern, Kind::Regex { case_insensitive }, view_arg(view)).is_match(value.as_bytes())
}

/// RE2 regex search on binary data.
#[pg_extern(immutable, parallel_safe, strict)]
fn wc_regex_bytes(value: &[u8], pattern: &str, case_insensitive: default!(bool, false)) -> bool {
    matcher(pattern, Kind::Regex { case_insensitive }, View::Raw).is_match(value)
}

#[pg_extern(immutable, parallel_safe, strict)]
fn wc_host(value: &str) -> String {
    String::from_utf8_lossy(views::extract_host(value.as_bytes())).into_owned()
}

#[pg_extern(immutable, parallel_safe, strict)]
fn wc_reverse_domain(value: &str) -> String {
    String::from_utf8_lossy(&views::reverse_domain(value.as_bytes())).into_owned()
}

/// Percent-decode like ClickHouse `decodeURLComponent` (bytea: may be invalid UTF-8).
#[pg_extern(immutable, parallel_safe, strict)]
fn wc_url_decode(value: &str) -> Vec<u8> {
    views::percent_decode(value.as_bytes())
}

/// Apply a view (display; invalid UTF-8 replaced).
#[pg_extern(immutable, parallel_safe, strict)]
fn wc_view(value: &str, view: &str) -> String {
    String::from_utf8_lossy(&view_arg(view).apply(value.as_bytes())).into_owned()
}

/// Trigram plan used for pruning (`ALL` = no pruning possible).
#[pg_extern(immutable, parallel_safe, strict)]
fn wc_explain(
    pattern: &str,
    kind: default!(&str, "'wildcard'"),
    view: default!(&str, "'raw'"),
) -> String {
    matcher(pattern, kind_arg(kind), view_arg(view))
        .plan
        .to_string()
}

/// RE2 portability problems of a regex (empty array when portable).
#[pg_extern(immutable, parallel_safe, strict)]
fn wc_re2_check(pattern: &str) -> Vec<String> {
    re2::check(pattern)
}

// ---------------------------------------------------------------- indicators

/// Undo defanging: `hxxp://evil[.]com` -> `http://evil.com`.
#[pg_extern(immutable, parallel_safe, strict)]
fn wc_refang(value: &str) -> String {
    indicator::refang(value)
}

/// Classify and normalise an indicator; returns its canonical search spec.
#[pg_extern(immutable, parallel_safe, strict)]
fn wc_indicator(
    raw: &str,
) -> TableIterator<
    'static,
    (
        name!(itype, String),
        name!(value, String),
        name!(view, String),
        name!(regex, String),
    ),
> {
    let s = indicator::spec(raw);
    TableIterator::once((
        s.itype.to_string(),
        s.value,
        s.view.name().to_string(),
        s.regex,
    ))
}

// ---------------------------------------------------------------- ClickHouse SQL

/// ClickHouse expression for a view of a column.
#[pg_extern(immutable, parallel_safe, strict)]
fn wc_ch_view_expr(col: &str, view: &str) -> String {
    wildcard_ch::view_expr(&ch(wildcard_ch::ident(col)), view_arg(view))
}

/// ClickHouse boolean expression equivalent to wc_match / wc_like / wc_regex.
/// `materialized`: use `<col>_<view>` columns created by `wc_ch_view_columns_ddl`.
#[pg_extern(immutable, parallel_safe, strict)]
fn wc_ch_predicate(
    col: &str,
    pattern: &str,
    kind: default!(&str, "'wildcard'"),
    view: default!(&str, "'raw'"),
    materialized: default!(bool, false),
) -> String {
    ch(wildcard_ch::column_predicate(
        &cols_arg(col, materialized),
        kind_arg(kind),
        pattern,
        view_arg(view),
    ))
}

/// DDL adding materialized view columns and n-gram text indexes to a ClickHouse table.
#[pg_extern(immutable, parallel_safe, strict)]
fn wc_ch_view_columns_ddl(
    table: &str,
    col: &str,
    views: default!(&str, "'raw,lower,decoded,revdomain'"),
) -> SetOfIterator<'static, String> {
    SetOfIterator::new(ch(wildcard_ch::view_columns_ddl(
        table,
        col,
        &views_arg(views),
    )))
}

/// IOC sweep queries (batched) returning `(select_cols..., indicator_id, view)`.
/// Pair arrays `ids`, `regexes`, `views` usually come from `wc_indicator()`.
#[pg_extern(immutable, parallel_safe)]
#[allow(clippy::too_many_arguments)]
fn wc_ch_sweep(
    source: &str,
    select_cols: Vec<String>,
    col: &str,
    ids: Vec<i64>,
    regexes: Vec<String>,
    views: Vec<String>,
    filter: default!(Option<&str>, "NULL"),
    materialized: default!(bool, false),
) -> SetOfIterator<'static, String> {
    if ids.len() != regexes.len() || ids.len() != views.len() {
        error!("pg_wildcard: ids, regexes and views must have the same length");
    }
    let items: Vec<SweepItem> = ids
        .iter()
        .zip(&regexes)
        .zip(&views)
        .map(|((&id, r), v)| SweepItem {
            id,
            regex: r.clone(),
            view: view_arg(v),
        })
        .collect();
    let sel: Vec<&str> = select_cols.iter().map(String::as_str).collect();
    let qs = ch(wildcard_ch::sweep_queries(
        source,
        &sel,
        &cols_arg(col, materialized),
        &items,
        filter,
        200_000,
    ));
    SetOfIterator::new(qs)
}

/// Sweep queries with explicit byte/AST budgets and scan diagnostics.
#[pg_extern(immutable, parallel_safe)]
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn wc_ch_sweep_plan(
    source: &str,
    select_cols: Vec<String>,
    col: &str,
    ids: Vec<i64>,
    regexes: Vec<String>,
    views: Vec<String>,
    max_ast_nodes: default!(i32, 45000),
    max_query_bytes: default!(i32, 200000),
    filter: default!(Option<&str>, "NULL"),
    materialized: default!(bool, false),
) -> TableIterator<
    'static,
    (
        name!(query, String),
        name!(strategy, String),
        name!(source_scans, i64),
        name!(estimated_ast_nodes, i64),
        name!(exact_only_items, i64),
    ),
> {
    if ids.len() != regexes.len() || ids.len() != views.len() {
        error!("pg_wildcard: ids, regexes and views must have the same length");
    }
    if max_ast_nodes <= 0 || max_query_bytes <= 0 {
        error!("pg_wildcard: sweep budgets must be positive");
    }
    let items: Vec<_> = ids
        .into_iter()
        .zip(regexes)
        .zip(views)
        .map(|((id, regex), view)| SweepItem {
            id,
            regex,
            view: view_arg(&view),
        })
        .collect();
    let sel: Vec<_> = select_cols.iter().map(String::as_str).collect();
    let plan = ch(wildcard_ch::sweep_queries_with_options(
        source,
        &sel,
        &cols_arg(col, materialized),
        &items,
        filter,
        wildcard_ch::SweepOptions {
            max_bytes: max_query_bytes as usize,
            max_ast_nodes: max_ast_nodes as usize,
        },
    ));
    let scans = plan.source_scan_count as i64;
    let exact = plan.exact_only_items as i64;
    let strategy = plan.strategy.name().to_owned();
    TableIterator::new(
        plan.queries
            .into_iter()
            .zip(plan.estimated_ast)
            .map(move |(query, nodes)| (query, strategy.clone(), scans, nodes as i64, exact)),
    )
}

/// DDL for a ClickHouse watchlist: indicator table + materialized view writing hits.
#[pg_extern(immutable, parallel_safe)]
#[allow(clippy::too_many_arguments)]
fn wc_ch_watchlist_ddl(
    name: &str,
    source: &str,
    hits: &str,
    watch: &str,
    select_cols: Vec<String>,
    col: &str,
    views: default!(&str, "'raw,lower,decoded,revdomain'"),
    materialized: default!(bool, false),
) -> SetOfIterator<'static, String> {
    let sel: Vec<&str> = select_cols.iter().map(String::as_str).collect();
    let mv = ch(wildcard_ch::watchlist_mv(
        name,
        source,
        hits,
        watch,
        &sel,
        &cols_arg(col, materialized),
        &views_arg(views),
    ));
    SetOfIterator::new(vec![ch(wildcard_ch::watchlist_table_ddl(watch)), mv])
}

/// Watchlist DDL with explicit strategy selection and conservative fallback.
#[pg_extern(immutable, parallel_safe)]
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn wc_ch_watchlist_plan(
    name: &str,
    source: &str,
    hits: &str,
    watch: &str,
    select_cols: Vec<String>,
    col: &str,
    views: default!(&str, "'raw,lower,decoded,revdomain'"),
    requested_strategy: default!(&str, "'staged_needles'"),
    materialized: default!(bool, false),
) -> TableIterator<
    'static,
    (
        name!(watchlist_ddl, String),
        name!(mv_ddl, String),
        name!(strategy, String),
        name!(fallback, Option<String>),
        name!(max_value_bytes, Option<i32>),
        name!(max_source_keys, Option<i32>),
    ),
> {
    let strategy = match requested_strategy {
        "staged_needles" => wildcard_ch::WatchlistStrategy::StagedNeedles,
        "rule_anchor_join" => wildcard_ch::WatchlistStrategy::RuleAnchorJoin,
        _ => error!("pg_wildcard: unknown watchlist strategy"),
    };
    let sel: Vec<_> = select_cols.iter().map(String::as_str).collect();
    let plan = ch(wildcard_ch::watchlist_plan(
        name,
        source,
        hits,
        watch,
        &sel,
        &cols_arg(col, materialized),
        &views_arg(views),
        strategy,
    ));
    TableIterator::new(std::iter::once((
        ch(wildcard_ch::watchlist_table_ddl(watch)),
        plan.sql,
        plan.strategy.name().to_owned(),
        plan.fallback.map(|f| f.name().to_owned()),
        plan.max_value_bytes.map(|n| n as i32),
        plan.max_source_keys.map(|n| n as i32),
    )))
}

/// Insert a watchlist indicator with conservative needles bound to its source.
#[pg_extern(immutable, parallel_safe, strict)]
fn wc_ch_watchlist_insert(
    watch: &str,
    id: i64,
    regex: &str,
    view: default!(&str, "'raw'"),
) -> String {
    ch(wildcard_ch::watchlist_insert_sql(
        watch,
        &SweepItem {
            id,
            regex: regex.to_owned(),
            view: view_arg(view),
        },
    ))
}

// ---------------------------------------------------------------- lake pruning

extension_sql!(
    r#"
CREATE TABLE wc_lake_index (
    index_name  text PRIMARY KEY,
    column_name text NOT NULL,
    views       text NOT NULL,
    snapshot_id bigint NOT NULL,
    loaded_at   timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE wc_lake_file (
    index_name    text NOT NULL REFERENCES wc_lake_index ON DELETE CASCADE,
    file_ord      int NOT NULL,
    path          text NOT NULL,
    record_count  bigint NOT NULL,
    has_unindexed boolean NOT NULL,
    PRIMARY KEY (index_name, file_ord)
);
CREATE TABLE wc_lake_gram (
    index_name text NOT NULL REFERENCES wc_lake_index ON DELETE CASCADE,
    gram       bigint NOT NULL,
    files      int4[] NOT NULL,
    PRIMARY KEY (index_name, gram)
);
COMMENT ON TABLE wc_lake_gram IS 'Puffin wildcard-ngram-v1 index loaded with: wc-lake export-pg, then \copy';
"#,
    name = "lake_tables"
);

struct LakeIndex {
    views: Vec<View>,
    paths: Vec<String>,
    unindexed: Vec<u32>,
}

fn lake_load(index_name: &str) -> LakeIndex {
    Spi::connect(|client| {
        let views: String = client
            .select(
                "SELECT views FROM wc_lake_index WHERE index_name = $1",
                Some(1),
                &[index_name.into()],
            )
            .unwrap_or_else(|e| error!("pg_wildcard: {e}"))
            .first()
            .get_one::<String>()
            .ok()
            .flatten()
            .unwrap_or_else(|| error!("pg_wildcard: unknown lake index '{index_name}'"));
        let mut paths = Vec::new();
        let mut unindexed = Vec::new();
        let rows = client
            .select(
                "SELECT file_ord, path, has_unindexed FROM wc_lake_file WHERE index_name = $1 ORDER BY file_ord",
                None,
                &[index_name.into()],
            )
            .unwrap_or_else(|e| error!("pg_wildcard: {e}"));
        for row in rows {
            let ord: i32 = row.get(1).ok().flatten().unwrap_or(-1);
            if ord as usize != paths.len() {
                error!("pg_wildcard: lake index '{index_name}' has non-contiguous file ordinals");
            }
            paths.push(row.get::<String>(2).ok().flatten().unwrap_or_default());
            if row.get::<bool>(3).ok().flatten().unwrap_or(true) {
                unindexed.push(ord as u32);
            }
        }
        LakeIndex {
            views: views_arg(&views),
            paths,
            unindexed,
        }
    })
}

pub(crate) fn lake_candidates(
    index_name: &str,
    pattern: &str,
    kind: Kind,
    view: View,
) -> (Vec<String>, bool) {
    let (_, candidates, used) = lake_candidates_with_coverage(index_name, pattern, kind, view);
    (candidates, used)
}

/// Retain the inventory that justifies exclusions alongside its candidates.
pub(crate) fn lake_candidates_with_coverage(
    index_name: &str,
    pattern: &str,
    kind: Kind,
    view: View,
) -> (Vec<String>, Vec<String>, bool) {
    let idx = lake_load(index_name);
    let m = matcher(pattern, kind, view);
    if !idx.views.contains(&view) || m.plan.is_all() {
        return (idx.paths.clone(), idx.paths, false);
    }
    let grams: Vec<i64> = m.plan.grams().into_iter().map(|g| g as i64).collect();
    let postings: HashMap<u64, Vec<u32>> = Spi::connect(|client| {
        let rows = client
            .select(
                "SELECT gram, files FROM wc_lake_gram WHERE index_name = $1 AND gram = ANY($2)",
                None,
                &[index_name.into(), grams.into()],
            )
            .unwrap_or_else(|e| error!("pg_wildcard: {e}"));
        rows.map(|r| {
            let g: i64 = r.get(1).ok().flatten().unwrap_or_default();
            let f: Vec<i32> = r.get(2).ok().flatten().unwrap_or_default();
            (g as u64, f.into_iter().map(|x| x as u32).collect())
        })
        .collect()
    });
    let mut ids = m.plan.candidates(idx.paths.len() as u32, &mut |g| {
        postings.get(&g).cloned().unwrap_or_default()
    });
    ids.extend(&idx.unindexed);
    ids.sort_unstable();
    ids.dedup();
    let candidates = ids
        .into_iter()
        .map(|i| idx.paths[i as usize].clone())
        .collect();
    (idx.paths, candidates, true)
}

/// Data files of a lake index that may contain matches (all files when the
/// pattern has no usable grams or the view is not indexed).
#[pg_extern(stable, strict)]
fn wc_lake_candidates(
    index_name: &str,
    pattern: &str,
    kind: default!(&str, "'wildcard'"),
    view: default!(&str, "'raw'"),
) -> SetOfIterator<'static, String> {
    SetOfIterator::new(lake_candidates(index_name, pattern, kind_arg(kind), view_arg(view)).0)
}

/// Antalya/ClickHouse query excluding only known impossible immutable files.
/// Files outside the imported inventory always remain eligible.
#[pg_extern(stable)]
#[allow(clippy::too_many_arguments)]
fn wc_lake_sql(
    index_name: &str,
    lake_table: &str,
    select_list: &str,
    pattern: &str,
    kind: default!(&str, "'wildcard'"),
    view: default!(&str, "'raw'"),
    extra_where: default!(Option<&str>, "NULL"),
    materialized: default!(bool, false),
) -> Option<String> {
    Some(lake_sql(
        index_name,
        lake_table,
        select_list,
        pattern,
        kind,
        view,
        extra_where,
        materialized,
        None,
    ))
}

/// Coverage-bound server inventory keeps selective guards small on large lakes.
#[pg_extern(stable)]
#[allow(clippy::too_many_arguments)]
fn wc_lake_sql_with_inventory(
    index_name: &str,
    lake_table: &str,
    select_list: &str,
    pattern: &str,
    known_table: &str,
    kind: default!(&str, "'wildcard'"),
    view: default!(&str, "'raw'"),
    extra_where: default!(Option<&str>, "NULL"),
    materialized: default!(bool, false),
) -> String {
    lake_sql(
        index_name,
        lake_table,
        select_list,
        pattern,
        kind,
        view,
        extra_where,
        materialized,
        Some(known_table),
    )
}

#[allow(clippy::too_many_arguments)]
fn lake_sql(
    index_name: &str,
    lake_table: &str,
    select_list: &str,
    pattern: &str,
    kind: &str,
    view: &str,
    extra_where: Option<&str>,
    materialized: bool,
    known_table: Option<&str>,
) -> String {
    let (k, v) = (kind_arg(kind), view_arg(view));
    let column: String = Spi::get_one_with_args(
        "SELECT column_name FROM wc_lake_index WHERE index_name = $1",
        &[index_name.into()],
    )
    .ok()
    .flatten()
    .unwrap_or_else(|| error!("pg_wildcard: unknown lake index '{index_name}'"));
    let (indexed, files, _) = lake_candidates_with_coverage(index_name, pattern, k, v);
    let pred = ch(wildcard_ch::column_predicate(
        &cols_arg(&column, materialized),
        k,
        pattern,
        v,
    ));
    let (query, strategy) = ch(wildcard_ch::lake_query_with_inventory(
        lake_table,
        select_list,
        &pred,
        &indexed,
        &files,
        extra_where,
        known_table,
    ));
    if strategy == wildcard_ch::LakeFilterStrategy::FilterBudgetExceeded {
        format!("/* pg_wildcard: {} */\n{query}", strategy.description())
    } else {
        query
    }
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;

    fn b(sql: &str) -> bool {
        Spi::get_one::<bool>(sql).unwrap().unwrap()
    }
    fn s(sql: &str) -> String {
        Spi::get_one::<String>(sql).unwrap().unwrap()
    }

    #[pg_test]
    fn clickhouse_semantics_locally() {
        assert!(b(
            "SELECT wc_match('GET /blog/%77p-ADMIN/setup.PHP', '*/wp-admin/*.php', 'decoded')"
        ));
        assert!(b("SELECT wc_like('aéb', 'a_b')")); // one character, not one byte
        assert!(b("SELECT wc_regex(E'a\\nb', 'a.b')")); // `.` matches newline
        assert!(!b("SELECT wc_regex('٣', '\\d')")); // ASCII \d
        assert!(b(
            "SELECT wc_like('https://login.EVIL.com/x', 'com.evil.%', 'revdomain')"
        ));
        assert!(!b("SELECT wc_regex(repeat('a', 50000) || '!', '(a+)+$')"));
    }

    #[pg_test]
    fn matcher_cache_keeps_dynamic_pattern_view_and_regex_flags_distinct() {
        assert!(b(
            "SELECT bool_and(wc_match(v,p,view) = expected) FROM (VALUES
             ('ABC','a*','raw',false), ('ABC','a*','lower',true),
             ('ABC','A*','raw',true), ('ABC','a*','RAW',false),
             ('%41BC','a*','decoded',true), ('ABC','B*','lower',false),
             ('aéb','a?b','raw',true), ('','*','raw',true)
             ) AS t(v,p,view,expected)"
        ));
        assert!(b(
            "SELECT bool_and(wc_regex(v,p,ci,view) = expected) FROM (VALUES
             ('ABC','^a',false,'raw',false), ('ABC','^a',true,'raw',true),
             ('ABC','^a',false,'lower',true), ('ABC','^b',true,'raw',false),
             ('ABC','^a',false,'raw',false)
             ) AS t(v,p,ci,view,expected)"
        ));
        assert!(b(
            "SELECT bool_and(wc_like(v,p,view) = expected) FROM (VALUES
             ('a%b','a\\%b','raw',true), ('axb','a\\%b','raw',false),
             ('aéb','a_b','raw',true), ('A','a','lower',true)
             ) AS t(v,p,view,expected)"
        ));
    }

    #[pg_test]
    fn matcher_cache_eviction_keeps_sql_results() {
        assert!(b("SELECT wc_match('first', 'first', 'raw')"));
        assert!(b("SELECT bool_and(wc_match(i::text, i::text, 'raw'))
             FROM generate_series(1,300) AS t(i)"));
        assert!(b("SELECT wc_match('first', 'first', 'raw')"));
        assert!(!b("SELECT wc_match('second', 'first', 'raw')"));
    }

    #[pg_test]
    fn matcher_cache_recovers_after_postgres_catches_compile_error() {
        assert!(b("SELECT wc_regex('abc', '^a')"));
        Spi::run(
            "DO $$ BEGIN
               BEGIN
                 PERFORM wc_regex('abc','[');
                 RAISE EXCEPTION 'invalid regex unexpectedly accepted';
               EXCEPTION WHEN OTHERS THEN
                 IF SQLERRM NOT LIKE 'pg_wildcard:%' THEN RAISE; END IF;
               END;
             END $$;",
        )
        .unwrap();
        assert!(b("SELECT wc_regex('abc', '^a')"));
        assert!(!b("SELECT wc_regex('abc', '^z')"));
    }

    #[pg_test]
    fn indicators() {
        assert_eq!(
            s("SELECT wc_refang('hxxp://evil[.]com')"),
            "http://evil.com"
        );
        assert_eq!(
            s("SELECT itype || ' ' || view || ' ' || regex FROM wc_indicator('Login.Evil[.]com.')"),
            r"domain revdomain ^com\.evil\.login(\.|$)"
        );
        assert_eq!(
            Spi::get_one::<i64>("SELECT count(*) FROM unnest(wc_re2_check('[a&&b]'))")
                .unwrap()
                .unwrap(),
            1
        );
    }

    #[pg_test]
    fn sweep_diagnostics_use_explicit_budgets() {
        assert!(b(
            "SELECT bool_and(source_scans = 1 AND estimated_ast_nodes > 0 AND strategy = 'constant_candidates') FROM wc_ch_sweep_plan('logs', ARRAY['id'], 'url', ARRAY[1,2]::bigint[], ARRAY['abc','def'], ARRAY['raw','raw'])"
        ));
        assert!(b(
            "SELECT bool_and(source_scans <= 2 AND estimated_ast_nodes <= 45000) FROM wc_ch_sweep_plan('logs', ARRAY['id'], 'url', ARRAY(SELECT n::bigint FROM generate_series(0,999) n), ARRAY(SELECT 'evil' || lpad(n::text,4,'0') || '\\.example' FROM generate_series(0,999) n), array_fill('raw'::text,ARRAY[1000]))"
        ));
    }

    #[pg_test]
    fn watchlist_strategy_is_explicit_and_reports_fallback() {
        assert!(b(
            "SELECT strategy = 'staged_needles' AND fallback IS NULL AND max_source_keys IS NULL \
                   FROM wc_ch_watchlist_plan('mv','logs','hits','watch',ARRAY['id'],'url')"
        ));
        assert!(b(
            "SELECT strategy = 'rule_anchor_join' AND fallback IS NULL AND max_source_keys = 255 \
                   FROM wc_ch_watchlist_plan('mv','logs','hits','watch',ARRAY['id'],'url','raw','rule_anchor_join')"
        ));
        assert!(b(
            "SELECT strategy = 'staged_needles' AND fallback = 'requires_single_raw_view' \
                   FROM wc_ch_watchlist_plan('mv','logs','hits','watch',ARRAY['id'],'url','raw,lower','rule_anchor_join')"
        ));
    }

    #[pg_test(error = "pg_wildcard: unknown watchlist strategy")]
    fn watchlist_strategy_rejects_typos() {
        Spi::run("SELECT * FROM wc_ch_watchlist_plan('mv','logs','hits','watch',ARRAY['id'],'url','raw','join')").unwrap();
    }

    #[pg_test]
    fn clickhouse_sql() {
        assert_eq!(
            s("SELECT wc_ch_predicate('url', '*/wp_admin/*.PHP', 'wildcard', 'lower')"),
            r"like(lower(url), '%/wp\\_admin/%.php')"
        );
        assert_eq!(
            s("SELECT wc_ch_predicate('url', '*.php', 'wildcard', 'decoded', true)"),
            r"like(url_decoded, '%.php')"
        );
        let n = Spi::get_one::<i64>("SELECT count(*) FROM wc_ch_view_columns_ddl('logs', 'url')")
            .unwrap()
            .unwrap();
        assert_eq!(n, 11);
        let q = s(
            "SELECT string_agg(q, ';') FROM wc_ch_sweep('logs', ARRAY['id'], 'url', ARRAY[1,2]::bigint[], ARRAY['evil\\.com','sqlmap'], ARRAY['lower','lower']) q",
        );
        assert!(
            q.contains("match(lower(url), 'sqlmap')")
                && !q.contains("multiMatchAllIndices")
                && !q.contains("wc_pats_"),
            "{q}"
        );
        let n = Spi::get_one::<i64>("SELECT count(*) FROM wc_ch_watchlist_ddl('mv', 'logs', 'hits', 'watch', ARRAY['id'], 'url')").unwrap().unwrap();
        assert_eq!(n, 2);
        let insert = s("SELECT wc_ch_watchlist_insert('watch', 7, 'abc', 'lower')");
        assert_eq!(
            insert,
            "INSERT INTO watch (id, view, regex, needles, needles_regex, needles_view) VALUES (7, 'lower', 'abc', ['abc'], 'abc', 'lower')"
        );
    }

    #[pg_test]
    fn planner_rewrite_keeps_semantics() {
        Spi::run("CREATE TEMP TABLE t(u text)").unwrap();
        Spi::run("INSERT INTO t VALUES (E'a\\nb'), ('aéb'), ('a%b'), ('a_b'), ('ab'), (E'x\\\\y')")
            .unwrap();
        let n = |sql: &str| Spi::get_one::<i64>(sql).unwrap().unwrap();
        // rewritten to LIKE (column argument) vs evaluated by wc_* (constant argument)
        for (pat, kind) in [
            ("a?b", "wc_match"),
            ("a_b", "wc_like"),
            ("a\\%b", "wc_like"),
            ("*\\\\*", "wc_match"),
            ("a*", "wc_match"),
        ] {
            let rewritten = n(&format!("SELECT count(*) FROM t WHERE {kind}(u, '{pat}')"));
            let direct = n(&format!(
                "SELECT count(*) FROM t WHERE {kind}(u || '', '{pat}', 'raw')"
            ));
            assert_eq!(rewritten, direct, "{kind} {pat}");
        }
        let plan: String = Spi::connect(|c| {
            c.select(
                "EXPLAIN (COSTS OFF) SELECT * FROM t WHERE wc_match(u, '*é*')",
                None,
                &[],
            )
            .unwrap()
            .map(|r| r.get::<String>(1).unwrap().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
        });
        assert!(plan.contains("~~"), "{plan}");
    }

    #[pg_test(error = "pg_wildcard: invalid identifier 'x; DROP TABLE y'")]
    fn rejects_injection() {
        Spi::run("SELECT wc_ch_predicate('x; DROP TABLE y', 'a')").unwrap();
    }

    #[pg_test]
    fn lake_pruning() {
        // A tiny index: file 0 has "evil", file 1 has "good"; grams from wc_explain's plan.
        Spi::run("INSERT INTO wc_lake_index VALUES ('t', 'url', 'raw', 1)").unwrap();
        Spi::run("INSERT INTO wc_lake_file VALUES ('t', 0, 's3://l/f0.parquet', 10, false), ('t', 1, 's3://l/f1.parquet', 10, false), ('t', 2, 's3://l/f2.parquet', 10, true)").unwrap();
        // index every raw gram of the two sample values
        let grams =
            |v: &str| wildcard_core::grams::value_grams(wildcard_core::View::Raw, v.as_bytes());
        let mut all: std::collections::BTreeMap<u64, Vec<i32>> = Default::default();
        for (ord, v) in [(0, "https://evil.com/x"), (1, "https://good.example/y")] {
            for g in grams(v) {
                all.entry(g).or_default().push(ord);
            }
        }
        for (g, files) in all {
            let arr = files
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(",");
            Spi::run(&format!(
                "INSERT INTO wc_lake_gram VALUES ('t', {}, '{{{arr}}}')",
                g as i64
            ))
            .unwrap();
        }
        let c =
            s("SELECT string_agg(p, ',' ORDER BY p) FROM wc_lake_candidates('t', '*evil.com*') p");
        assert_eq!(c, "s3://l/f0.parquet,s3://l/f2.parquet"); // f2 has unindexed values
        let q = s("SELECT wc_lake_sql('t', 'lake.http', 'url', '*evil.com*')");
        assert!(
            q.contains("_path NOT IN ('s3://l/f1.parquet')")
                && q.contains("like(url, '%evil.com%')"),
            "{q}"
        );
        let c = s("SELECT count(*)::text FROM wc_lake_candidates('t', '*ab*')");
        assert_eq!(c, "3"); // no usable gram: every file
    }

    #[pg_test]
    fn lake_sql_keeps_appended_files_and_exact_rechecks() {
        Spi::run("INSERT INTO wc_lake_index VALUES ('coverage', 'url', 'raw', 1)").unwrap();
        Spi::run("INSERT INTO wc_lake_file VALUES ('coverage', 0, 'old', 1, false), ('coverage', 1, 'unindexed', 1, true)").unwrap();
        Spi::run("CREATE TEMP TABLE lake_current (_path text, url text)").unwrap();
        Spi::run("INSERT INTO lake_current VALUES ('old', 'ordinary'), ('unindexed', 'evil.test'), ('new', 'evil.test'), ('new', 'ordinary')").unwrap();

        // PostgreSQL also accepts these path/LIKE expressions, so execute the
        // generated query to test rows, including the exact matching recheck.
        let query = s("SELECT wc_lake_sql('coverage', 'lake_current', '_path', '*evil.test*')");
        assert_eq!(
            s(&format!(
                "SELECT string_agg(_path, ',' ORDER BY _path) FROM ({query}) q"
            )),
            "new,unindexed"
        );

        // No indexed candidates still says nothing about newer files.
        Spi::run("DELETE FROM lake_current WHERE _path = 'unindexed'").unwrap();
        Spi::run("UPDATE wc_lake_file SET has_unindexed = false WHERE index_name = 'coverage'")
            .unwrap();
        let query = s("SELECT wc_lake_sql('coverage', 'lake_current', '_path', '*evil.test*')");
        assert_eq!(s(&query), "new");

        // Empty inventory, unavailable view, and gram-free patterns all widen.
        Spi::run("DELETE FROM wc_lake_file WHERE index_name = 'coverage'").unwrap();
        let query = s("SELECT wc_lake_sql('coverage', 'lake_current', '_path', '*evil.test*')");
        assert_eq!(s(&query), "new");
        Spi::run("INSERT INTO wc_lake_file VALUES ('coverage', 0, 'old', 1, false)").unwrap();
        for args in ["'*evil.test*', 'wildcard', 'lower'", "'*il*'"] {
            let query = s(&format!(
                "SELECT wc_lake_sql('coverage', 'lake_current', '_path', {args})"
            ));
            assert!(!query.contains("_path NOT IN"), "{query}");
            assert_eq!(s(&query), "new");
        }
    }

    #[pg_test]
    fn lake_sql_widens_when_path_filter_exceeds_budget() {
        Spi::run("INSERT INTO wc_lake_index VALUES ('large', 'url', 'raw', 1)").unwrap();
        Spi::run("INSERT INTO wc_lake_file SELECT 'large', 0, repeat('x', 65536), 1, false")
            .unwrap();
        let query = s("SELECT wc_lake_sql('large', 'lake.http', 'url', '*evil.test*')");
        assert!(!query.contains("_path"), "oversized guard must widen");
        assert!(query.contains("like(url, '%evil.test%')"));
        assert!(query.contains("pruning disabled:"));
    }

    #[pg_test]
    fn lake_sql_uses_bound_remote_inventory_for_ten_thousand_paths() {
        Spi::run("INSERT INTO wc_lake_index VALUES ('remote_scale', 'url', 'raw', 1)").unwrap();
        Spi::run("INSERT INTO wc_lake_file SELECT 'remote_scale', n, 's3://bucket/' || repeat('x', 90) || '/' || n || '.parquet', 1, n = 0 FROM generate_series(0,9999) n").unwrap();
        let query = s(
            "SELECT wc_lake_sql_with_inventory('remote_scale', 'events', 'url', '*needle-marker*', 'inventory.known')",
        );
        assert!(query.len() < 65536, "selective guard must stay bounded");
        assert!(
            query.contains(
                "NOT IN (SELECT path FROM inventory.known WHERE index_name = 'wc-lake-v1:"
            ),
            "{query}"
        );
        assert!(
            query.contains(" OR _path IN ("),
            "unknown and candidate files remain eligible"
        );
        assert!(query.contains("like(url, '%needle-marker%')"));
        let fallback = s("SELECT wc_lake_sql('remote_scale', 'events', 'url', '*needle-marker*')");
        assert!(fallback.contains("pruning disabled:"));
    }
}

/// Required by `cargo pgrx test`.
#[cfg(test)]
pub mod pg_test {
    pub fn setup(_options: Vec<&str>) {}

    #[must_use]
    pub fn postgresql_conf_options() -> Vec<&'static str> {
        vec![]
    }
}
