// SPDX-License-Identifier: Apache-2.0
//! `wildcard_fdw`: a scan-only foreign data wrapper for ClickHouse/Antalya,
//! specialised for security search.
//!
//! It pushes down what general-purpose FDWs cannot: `wc_match`, `wc_like` and
//! `wc_regex` calls (compiled by `wildcard-ch` into exactly equivalent
//! ClickHouse SQL, optionally on materialized view columns), plus simple
//! comparisons, `LIKE` and NULL tests; and it prunes Iceberg data files with
//! the Puffin n-gram index (excluding only known non-candidates). Every clause is also kept as a
//! local qual, preventing extra rows from an over-selecting remote predicate. Exact
//! pushdown is required separately to avoid false negatives.
//!
//! Server options: `url` (http://host:8123), `database`, `settings`
//! (`k=v,k=v`, from a narrow allowlist), `timeout_ms`. User mapping option:
//! `bearer_token` (legacy `user`/`password` is refused). Table options: `table` (ClickHouse table or table function),
//! `materialized_views` (`true`: use `<col>_<view>` columns), `lake_index`
//! (name in `wc_lake_index`), `lake_known_table` (optional content-addressed
//! ClickHouse inventory from `wc-lake export-pg`), `rows` (row estimate).

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::collections::{BTreeSet, HashMap};
use std::ffi::{CStr, CString};
mod datum;
mod planning;
mod predicates;
mod transport;
mod values;
use antalya_rs::BudgetedBatch;

use pgrx::pg_sys;
use pgrx::prelude::*;
use pgrx::{FromDatum, PgList, PgMemoryContexts};
use wildcard_ch::ViewColumns;
use wildcard_core::View;
use wildcard_core::verify::Kind;

// ------------------------------------------------------------------ options

unsafe fn read_options(list: *mut pg_sys::List, out: &mut HashMap<String, String>) {
    unsafe {
        for de in PgList::<pg_sys::DefElem>::from_pg(list).iter_ptr() {
            let name = CStr::from_ptr((*de).defname).to_string_lossy().into_owned();
            let val = CStr::from_ptr(pg_sys::defGetString(de))
                .to_string_lossy()
                .into_owned();
            out.insert(name, val);
        }
    }
}

/// Server, then user mapping, then table options (later ones win).
unsafe fn options(relid: pg_sys::Oid) -> HashMap<String, String> {
    unsafe {
        let mut out = HashMap::new();
        let table = pg_sys::GetForeignTable(relid);
        let server = pg_sys::GetForeignServer((*table).serverid);
        read_options((*server).options, &mut out);
        if out.keys().any(|key| {
            !matches!(
                key.as_str(),
                "url" | "database" | "settings" | "timeout_ms" | "terminal_policy"
            )
        }) {
            error!(
                "wildcard_fdw: unsupported server option; authentication belongs in user mappings"
            );
        }
        let um = pg_sys::GetUserMapping(pg_sys::GetUserId(), (*table).serverid);
        let mut mapping = HashMap::new();
        read_options((*um).options, &mut mapping);
        if mapping
            .keys()
            .any(|key| !matches!(key.as_str(), "bearer_token" | "user" | "password"))
        {
            error!("wildcard_fdw: unsupported user mapping option");
        }
        out.extend(mapping);
        let mut table_options = HashMap::new();
        read_options((*table).options, &mut table_options);
        if table_options.keys().any(|key| {
            !matches!(
                key.as_str(),
                "table"
                    | "materialized_views"
                    | "lake_index"
                    | "lake_known_table"
                    | "rows"
                    | "settings"
                    | "candidate_columns"
                    | "candidate_prewhere"
                    | "candidate_mode"
            )
        }) {
            error!(
                "wildcard_fdw: unsupported table option; authentication belongs in user mappings"
            );
        }
        if let Some(rows) = table_options.get("rows")
            && rows
                .parse::<f64>()
                .ok()
                .is_none_or(|n| !n.is_finite() || n < 0.0)
        {
            error!("wildcard_fdw: invalid rows estimate");
        }
        out.extend(table_options);
        out
    }
}

fn opt_bool(o: &HashMap<String, String>, k: &str) -> bool {
    o.get(k)
        .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "on" | "1" | "yes"))
}

// ------------------------------------------------------------------ deparse

/// Backtick-quoted ClickHouse identifier.
fn ch_ident(name: &str) -> String {
    format!("`{}`", name.replace('\\', "\\\\").replace('`', "\\`"))
}

unsafe fn strip_relabel(mut n: *mut pg_sys::Node) -> *mut pg_sys::Node {
    unsafe {
        while !n.is_null() && (*n).type_ == pg_sys::NodeTag::T_RelabelType {
            n = (*(n as *mut pg_sys::RelabelType)).arg as *mut pg_sys::Node;
        }
        n
    }
}

/// Column name if `n` is a Var of the scanned relation.
unsafe fn var_column(n: *mut pg_sys::Node, relid: pg_sys::Oid, scanrelid: u32) -> Option<String> {
    unsafe {
        let n = strip_relabel(n);
        if n.is_null() || (*n).type_ != pg_sys::NodeTag::T_Var {
            return None;
        }
        let v = n as *mut pg_sys::Var;
        if (*v).varno as u32 != scanrelid || (*v).varattno <= 0 || (*v).varlevelsup != 0 {
            return None;
        }
        let name = pg_sys::get_attname(relid, (*v).varattno, false);
        Some(CStr::from_ptr(name).to_string_lossy().into_owned())
    }
}

/// Text form of a non-null Const plus its type.
unsafe fn const_text(n: *mut pg_sys::Node) -> Option<(String, pg_sys::Oid)> {
    unsafe {
        let n = strip_relabel(n);
        if n.is_null() || (*n).type_ != pg_sys::NodeTag::T_Const {
            return None;
        }
        let c = n as *mut pg_sys::Const;
        if (*c).constisnull {
            return None;
        }
        match (*c).consttype {
            pg_sys::TIMESTAMPOID | pg_sys::TIMESTAMPTZOID => {
                let micros = predicates::timestamp(i64::from_datum((*c).constvalue, false)?)?;
                return Some((micros.to_string(), (*c).consttype));
            }
            pg_sys::DATEOID => {
                let date = predicates::date(i32::from_datum((*c).constvalue, false)?)?;
                return Some((date, (*c).consttype));
            }
            _ => {}
        }
        let mut out_fn = pg_sys::InvalidOid;
        let mut varlena = false;
        pg_sys::getTypeOutputInfo((*c).consttype, &mut out_fn, &mut varlena);
        let s = pg_sys::OidOutputFunctionCall(out_fn, (*c).constvalue);
        Some((
            CStr::from_ptr(s).to_string_lossy().into_owned(),
            (*c).consttype,
        ))
    }
}

fn ch_literal(text: &str, ty: pg_sys::Oid) -> Option<String> {
    match ty {
        pg_sys::INT2OID
        | pg_sys::INT4OID
        | pg_sys::INT8OID
        | pg_sys::FLOAT4OID
        | pg_sys::FLOAT8OID
        | pg_sys::NUMERICOID => {
            text.parse::<f64>().ok().filter(|f| f.is_finite())?;
            Some(text.to_string())
        }
        pg_sys::BOOLOID => Some(if text == "t" {
            "true".into()
        } else {
            "false".into()
        }),
        pg_sys::TEXTOID | pg_sys::VARCHAROID | pg_sys::NAMEOID => Some(wildcard_ch::quote(text)),
        pg_sys::TIMESTAMPOID | pg_sys::TIMESTAMPTZOID => Some(format!(
            "fromUnixTimestamp64Micro({}, 'UTC')",
            text.parse::<i64>().ok()?
        )),
        pg_sys::DATEOID => Some(format!("toDate32({})", wildcard_ch::quote(text))),
        _ => None,
    }
}

/// What a pushed wc_* call needs for lake pruning.
pub(crate) struct WcCall {
    pub column: String,
    pub kind: Kind,
    pub pattern: String,
    pub view: View,
}

/// Recognise `wc_match(col, 'p', 'view')`, `wc_like(...)`, `wc_regex(col, 'p', ci, 'view')`.
unsafe fn wc_call(n: *mut pg_sys::Node, relid: pg_sys::Oid, scanrelid: u32) -> Option<WcCall> {
    unsafe {
        if n.is_null() || (*n).type_ != pg_sys::NodeTag::T_FuncExpr {
            return None;
        }
        let f = n as *mut pg_sys::FuncExpr;
        let extension = pg_sys::get_extension_oid(c"pg_wildcard".as_ptr(), true);
        if extension == pg_sys::InvalidOid
            || pg_sys::getExtensionOfObject(pg_sys::ProcedureRelationId, (*f).funcid) != extension
        {
            return None;
        }
        let name = CStr::from_ptr(pg_sys::get_func_name((*f).funcid))
            .to_string_lossy()
            .into_owned();
        let args: Vec<*mut pg_sys::Node> = PgList::<pg_sys::Node>::from_pg((*f).args)
            .iter_ptr()
            .collect();
        let column = var_column(*args.first()?, relid, scanrelid)?;
        let (pattern, _) = const_text(*args.get(1)?)?;
        let (kind, view_arg) = match (name.as_str(), args.len()) {
            ("wc_match", 3) => (Kind::Wildcard, 2),
            ("wc_like", 3) => (Kind::Like, 2),
            ("wc_regex", 4) => {
                let (ci, _) = const_text(args[2])?;
                (
                    Kind::Regex {
                        case_insensitive: ci == "t",
                    },
                    3,
                )
            }
            _ => return None,
        };
        let view: View = const_text(args[view_arg])?.0.parse().ok()?;
        Some(WcCall {
            column,
            kind,
            pattern,
            view,
        })
    }
}

/// `col LIKE 'p'` or `col_<view> LIKE 'p'` (as produced by the planner
/// rewrite in `support.rs`), seen as a LIKE on `index_column` in that view.
unsafe fn like_call(
    n: *mut pg_sys::Node,
    relid: pg_sys::Oid,
    scanrelid: u32,
    index_column: &str,
    materialized: bool,
) -> Option<WcCall> {
    unsafe {
        if n.is_null() || (*n).type_ != pg_sys::NodeTag::T_OpExpr {
            return None;
        }
        let o = n as *mut pg_sys::OpExpr;
        if (*o).opno.to_u32() >= pg_sys::FirstNormalObjectId
            || !deterministic_collation((*o).inputcollid)
        {
            return None;
        }
        if CStr::from_ptr(pg_sys::get_opname((*o).opno)).to_bytes() != b"~~" {
            return None;
        }
        let args: Vec<*mut pg_sys::Node> = PgList::<pg_sys::Node>::from_pg((*o).args)
            .iter_ptr()
            .collect();
        let col = var_column(*args.first()?, relid, scanrelid)?;
        let (pattern, _) = const_text(*args.get(1)?)?;
        // Candidate and lake pruning must not hide a local invalid-escape
        // error after the exact deparser has declined this LIKE predicate.
        predicates::like(&pattern)?;
        let view = if col == index_column {
            View::Raw
        } else if materialized {
            View::ALL
                .into_iter()
                .find(|v| col == format!("{index_column}_{}", v.name()))?
        } else {
            return None;
        };
        Some(WcCall {
            column: index_column.to_string(),
            kind: Kind::Like,
            pattern,
            view,
        })
    }
}

/// Byte-sensitive native predicates cannot implement nondeterministic PG LIKE
/// or equality. This guard also gates candidate/Puffin extraction in like_call.
unsafe fn deterministic_collation(oid: pg_sys::Oid) -> bool {
    // SAFETY: inputcollid is a planner-owned catalog OID; unknown is conservative.
    oid != pg_sys::InvalidOid && unsafe { pg_sys::get_collation_isdeterministic(oid) }
}

/// Deparse a qual into ClickHouse SQL, or None if it cannot be pushed.
unsafe fn deparse(
    n: *mut pg_sys::Node,
    relid: pg_sys::Oid,
    scanrelid: u32,
    materialized: bool,
) -> Option<String> {
    unsafe {
        if n.is_null() {
            return None;
        }
        match (*n).type_ {
            pg_sys::NodeTag::T_BoolExpr => {
                let b = n as *mut pg_sys::BoolExpr;
                let parts: Option<Vec<String>> = PgList::<pg_sys::Node>::from_pg((*b).args)
                    .iter_ptr()
                    .map(|a| deparse(a, relid, scanrelid, materialized))
                    .collect();
                let parts = parts?;
                Some(match (*b).boolop {
                    pg_sys::BoolExprType::AND_EXPR => format!("({})", parts.join(" AND ")),
                    pg_sys::BoolExprType::OR_EXPR => format!("({})", parts.join(" OR ")),
                    _ => format!("(NOT {})", parts.first()?),
                })
            }
            pg_sys::NodeTag::T_OpExpr => {
                let o = n as *mut pg_sys::OpExpr;
                // Names alone are not identities: user operators may reuse '='
                // or LIKE names with different semantics, even builtin functions.
                if (*o).opno.to_u32() >= pg_sys::FirstNormalObjectId {
                    return None;
                }
                let args: Vec<*mut pg_sys::Node> = PgList::<pg_sys::Node>::from_pg((*o).args)
                    .iter_ptr()
                    .collect();
                if args.len() != 2 {
                    return None;
                }
                let op = CStr::from_ptr(pg_sys::get_opname((*o).opno))
                    .to_string_lossy()
                    .into_owned();
                let ch_op = match op.as_str() {
                    "=" | "<>" | "<" | ">" | "<=" | ">=" => op.clone(),
                    "~~" => "LIKE".into(),
                    "!~~" => "NOT LIKE".into(),
                    _ => return None,
                };
                let col = var_column(args[0], relid, scanrelid)?;
                let (text, ty) = const_text(args[1])?;
                // Cross-type text/name operators also carry locale semantics;
                // checking only a text RHS misses their builtin name overloads.
                if ((*o).inputcollid != pg_sys::InvalidOid
                    || matches!(ty, pg_sys::TEXTOID | pg_sys::VARCHAROID | pg_sys::NAMEOID))
                    && (!deterministic_collation((*o).inputcollid)
                        || (matches!(op.as_str(), "<" | ">" | "<=" | ">=")
                            && !planning::byte_collation((*o).inputcollid)))
                {
                    return None;
                }
                if ch_op.contains("LIKE") && !matches!(ty, pg_sys::TEXTOID | pg_sys::VARCHAROID) {
                    return None;
                }
                // PostgreSQL cross-type temporal comparisons can depend on the
                // session timezone; only same-type comparisons have this mapping.
                if matches!(
                    ty,
                    pg_sys::TIMESTAMPOID | pg_sys::TIMESTAMPTZOID | pg_sys::DATEOID
                ) && pg_sys::exprType(args[0]) != ty
                {
                    return None;
                }
                let literal = if ch_op.contains("LIKE") {
                    wildcard_ch::quote(&predicates::like(&text)?)
                } else {
                    ch_literal(&text, ty)?
                };
                Some(format!("{} {ch_op} {literal}", ch_ident(&col)))
            }
            pg_sys::NodeTag::T_NullTest => {
                let t = n as *mut pg_sys::NullTest;
                let col = var_column((*t).arg as *mut pg_sys::Node, relid, scanrelid)?;
                Some(if (*t).nulltesttype == pg_sys::NullTestType::IS_NULL {
                    format!("isNull({})", ch_ident(&col))
                } else {
                    format!("isNotNull({})", ch_ident(&col))
                })
            }
            pg_sys::NodeTag::T_FuncExpr => {
                let c = wc_call(n, relid, scanrelid)?;
                let cols = if materialized {
                    ViewColumns::conventional(&c.column)
                } else {
                    ViewColumns::new(&c.column)
                }
                .ok()?;
                wildcard_ch::column_predicate(&cols, c.kind, &c.pattern, c.view).ok()
            }
            _ => None,
        }
    }
}

/// Exclude known non-candidates, retaining unknown files. Intersect candidates
/// over every pushed wc_* call on the indexed column; bound the emitted guard.
/// Returns (condition, summary for EXPLAIN).
unsafe fn lake_filter(
    index: &str,
    known_table: Option<&str>,
    quals: &[*mut pg_sys::Node],
    relid: pg_sys::Oid,
    scanrelid: u32,
    materialized: bool,
) -> Option<(Option<String>, String)> {
    unsafe {
        let column: String = Spi::get_one_with_args(
            "SELECT column_name FROM wc_lake_index WHERE index_name = $1",
            &[index.into()],
        )
        .ok()
        .flatten()?;
        let mut indexed = Vec::new();
        let mut set: Option<BTreeSet<String>> = None;
        for &q in quals {
            let call = wc_call(q, relid, scanrelid)
                .or_else(|| like_call(q, relid, scanrelid, &column, materialized));
            if let Some(c) = call.filter(|c| c.column == column) {
                let (known, files, used) =
                    crate::lake_candidates_with_coverage(index, &c.pattern, c.kind, c.view);
                if used {
                    indexed = known;
                    let s: BTreeSet<String> = files.into_iter().collect();
                    set = Some(match set {
                        None => s,
                        Some(prev) => prev.intersection(&s).cloned().collect(),
                    });
                }
            }
        }
        let set = set?;
        let candidates: Vec<String> = set.into_iter().collect();
        let filter =
            wildcard_ch::lake_path_filter_with_inventory(&indexed, &candidates, known_table)
                .unwrap_or_else(|_| error!("wildcard_fdw: invalid lake inventory configuration"));
        let summary = format!(
            "{} of {} indexed files; unknown files retained (index {index}); {}",
            candidates.len(),
            indexed.len(),
            filter.strategy.description(),
        );
        Some((filter.condition, summary))
    }
}

// ------------------------------------------------------------------ planner callbacks

#[pg_guard]
unsafe extern "C-unwind" fn get_foreign_rel_size(
    _root: *mut pg_sys::PlannerInfo,
    baserel: *mut pg_sys::RelOptInfo,
    relid: pg_sys::Oid,
) {
    unsafe {
        let o = options(relid);
        (*baserel).rows = o.get("rows").and_then(|r| r.parse().ok()).unwrap_or(1000.0);
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn get_foreign_paths(
    root: *mut pg_sys::PlannerInfo,
    baserel: *mut pg_sys::RelOptInfo,
    _relid: pg_sys::Oid,
) {
    unsafe {
        let rows = (*baserel).rows;
        #[cfg(any(feature = "pg14", feature = "pg15", feature = "pg16"))]
        let path = pg_sys::create_foreignscan_path(
            root,
            baserel,
            std::ptr::null_mut(),
            rows,
            10.0,
            10.0 + rows,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        #[cfg(feature = "pg17")]
        let path = pg_sys::create_foreignscan_path(
            root,
            baserel,
            std::ptr::null_mut(),
            rows,
            10.0,
            10.0 + rows,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        #[cfg(feature = "pg18")]
        let path = pg_sys::create_foreignscan_path(
            root,
            baserel,
            std::ptr::null_mut(),
            rows,
            0,
            10.0,
            10.0 + rows,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        pg_sys::add_path(baserel, path as *mut pg_sys::Path);
    }
}

unsafe fn make_string(s: &str) -> *mut pg_sys::Node {
    unsafe {
        let c = CString::new(s.replace('\0', ""))
            .unwrap_or_else(|_| error!("wildcard_fdw: invalid internal string"));
        pg_sys::makeString(pg_sys::pstrdup(c.as_ptr())) as *mut pg_sys::Node
    }
}

unsafe fn str_val(n: *mut pg_sys::Node) -> String {
    // SAFETY: FDW private strings are allocated by make_string in PostgreSQL's
    // memory context. PostgreSQL 14 stores String nodes in the Value union.
    unsafe {
        #[cfg(feature = "pg14")]
        let value = (*(n as *mut pg_sys::Value)).val.str_;
        #[cfg(not(feature = "pg14"))]
        let value = (*(n as *mut pg_sys::String)).sval;
        CStr::from_ptr(value).to_string_lossy().into_owned()
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn get_foreign_plan(
    root: *mut pg_sys::PlannerInfo,
    baserel: *mut pg_sys::RelOptInfo,
    relid: pg_sys::Oid,
    _best_path: *mut pg_sys::ForeignPath,
    tlist: *mut pg_sys::List,
    scan_clauses: *mut pg_sys::List,
    outer_plan: *mut pg_sys::Plan,
) -> *mut pg_sys::ForeignScan {
    unsafe {
        let o = options(relid);
        let scanrelid = (*baserel).relid;
        let materialized = opt_bool(&o, "materialized_views");
        let quals: Vec<*mut pg_sys::Node> =
            PgList::<pg_sys::RestrictInfo>::from_pg((*baserel).baserestrictinfo)
                .iter_ptr()
                .map(|ri| (*ri).clause as *mut pg_sys::Node)
                .collect();
        let mut pushed: Vec<String> = quals
            .iter()
            .filter_map(|&q| deparse(q, relid, scanrelid, materialized))
            .collect();
        let mut lake_summary = String::new();
        if let Some(index) = o.get("lake_index")
            && let Some((cond, summary)) = lake_filter(
                index,
                o.get("lake_known_table").map(String::as_str),
                &quals,
                relid,
                scanrelid,
                materialized,
            )
        {
            if let Some(cond) = cond {
                pushed.insert(0, cond);
            }
            lake_summary = summary;
        }
        let automatic = match o
            .get("candidate_mode")
            .map(String::as_str)
            .unwrap_or("manual")
        {
            "manual" => false,
            "auto" => true,
            _ => error!("wildcard_fdw: invalid candidate_mode"),
        };
        let exact_where = if pushed.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", pushed.join(" AND "))
        };
        let mut candidates = Vec::new();
        if let Some(names) = o.get("candidate_columns") {
            if names.len() > 4096 {
                error!("wildcard_fdw: candidate_columns exceeds limit");
            }
            let names: Vec<_> = names.split(',').map(str::trim).collect();
            if names.len() > 64
                || names
                    .iter()
                    .any(|n| wildcard_ch::ident(n).is_err() || n.contains('.'))
            {
                error!("wildcard_fdw: invalid candidate_columns");
            }
            for &q in &quals {
                let call = wc_call(q, relid, scanrelid).or_else(|| {
                    names
                        .iter()
                        .find_map(|name| like_call(q, relid, scanrelid, name, materialized))
                });
                if let Some(call) = call {
                    let cols = if materialized {
                        ViewColumns::conventional(&call.column)
                    } else {
                        ViewColumns::new(&call.column)
                    };
                    if let Ok(cols) = cols {
                        let expr = cols.expr(call.view);
                        if names.contains(&expr.as_str())
                            && let Ok(Some(filter)) = wildcard_ch::candidate_predicate(
                                &expr,
                                call.kind,
                                &call.pattern,
                                call.view,
                            )
                        {
                            candidates.push(filter);
                        }
                    }
                }
            }
        }
        let prewhere = if !candidates.is_empty() && opt_bool(&o, "candidate_prewhere") {
            format!(" PREWHERE {}", candidates.join(" AND "))
        } else {
            pushed.extend(candidates);
            String::new()
        };
        let where_sql = if pushed.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", pushed.join(" AND "))
        };
        // Every qual is also re-checked locally: pushdown can only narrow results.
        let local = pg_sys::extract_actual_clauses(scan_clauses, false);
        let mut private = PgList::<pg_sys::Node>::new();
        let candidate_where = format!("{prewhere}{where_sql}");
        private.push(make_string(if automatic {
            &exact_where
        } else {
            &candidate_where
        }));
        private.push(make_string(&lake_summary));
        // PostgreSQL may supply an expanded physical tlist even for count(*).
        // reltarget describes the actual upper-plan dependencies; scan quals
        // add the columns needed by every retained local recheck.
        private.push(make_string(&planning::attributes(
            (*(*baserel).reltarget).exprs,
            local,
            scanrelid,
            relid,
        )));
        let topk = planning::ordered_limit(root, baserel, relid, scan_clauses, materialized)
            .unwrap_or_default();
        private.push(make_string(&topk));
        private.push(make_string(
            if automatic && candidate_where != exact_where {
                &candidate_where
            } else {
                ""
            },
        ));
        pg_sys::make_foreignscan(
            tlist,
            local,
            scanrelid,
            std::ptr::null_mut(),
            private.into_pg(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            outer_plan,
        )
    }
}

// ------------------------------------------------------------------ executor

struct Column {
    finfo: pg_sys::FmgrInfo,
    ioparam: pg_sys::Oid,
    typmod: i32,
    target: Option<values::Target>,
}

struct ScanState {
    sql: String,
    candidate_sql: Option<String>,
    settings: Option<String>,
    lake_summary: String,
    session: transport::Session,
    stream: Option<transport::Stream>,
    started: bool,
    batch: Option<BudgetedBatch>,
    schema: Option<arrow_schema::SchemaRef>,
    batch_row: usize,
    columns: Vec<Column>,
    rows: u64,
    metrics: transport::StreamMetrics,
    conversion: std::time::Duration,
    row_timing: bool,
}

/// Clock reads can dominate cheap row conversion. Keep their evaluation lazy
/// when PostgreSQL has not requested executor timing (including TIMING OFF).
fn conversion_timer<T>(enabled: bool, clock: impl FnOnce() -> T) -> Option<T> {
    enabled.then(clock)
}

fn target_type(oid: pg_sys::Oid, typmod: i32) -> Option<values::Target> {
    use values::Target::*;
    // Length/precision modifiers can silently truncate evidence in input functions.
    if typmod >= 0 {
        return None;
    }
    Some(match oid {
        pg_sys::TEXTOID | pg_sys::VARCHAROID => Text,
        pg_sys::BYTEAOID => Bytes,
        pg_sys::BOOLOID => Bool,
        pg_sys::INT2OID => Int16,
        pg_sys::INT4OID => Int32,
        pg_sys::INT8OID => Int64,
        pg_sys::FLOAT4OID => Float32,
        pg_sys::FLOAT8OID => Float64,
        pg_sys::DATEOID => Date,
        pg_sys::TIMESTAMPOID => Timestamp,
        pg_sys::TIMESTAMPTZOID => TimestampTz,
        _ => return None,
    })
}

#[pg_guard]
unsafe extern "C-unwind" fn begin_foreign_scan(node: *mut pg_sys::ForeignScanState, _eflags: i32) {
    // SAFETY: FDW callbacks execute on the PostgreSQL backend thread; PostgreSQL
    // owns node, relation descriptors, memory contexts and tuple slots.
    unsafe {
        if pg_sys::GetDatabaseEncoding() != pg_sys::pg_enc::PG_UTF8 as i32 {
            error!("wildcard_fdw: a UTF-8 PostgreSQL database is required");
        }
        let plan = (*node).ss.ps.plan as *mut pg_sys::ForeignScan;
        let private: Vec<*mut pg_sys::Node> = PgList::<pg_sys::Node>::from_pg((*plan).fdw_private)
            .iter_ptr()
            .collect();
        let rel = (*node).ss.ss_currentRelation;
        let relid = (*rel).rd_id;
        let o = options(relid);
        let table = o
            .get("table")
            .unwrap_or_else(|| error!("wildcard_fdw: table option 'table' is required"));
        // SAFETY: PostgreSQL keeps this relation-owned descriptor alive during
        // BeginForeignScan. Borrow it without freeing it or changing its refcount.
        let attrs = pgrx::PgTupleDesc::from_pg_unchecked((*rel).rd_att);
        let mut select = Vec::new();
        let mut columns = Vec::new();
        let projected: BTreeSet<usize> = str_val(private[2])
            .split(',')
            .filter(|n| !n.is_empty())
            .map(|n| {
                n.parse()
                    .unwrap_or_else(|_| error!("wildcard_fdw: invalid projection plan"))
            })
            .collect();
        for (position, a) in attrs.iter().enumerate() {
            let needed = !a.attisdropped && projected.contains(&(position + 1));
            let name = CStr::from_ptr(a.attname.data.as_ptr())
                .to_string_lossy()
                .into_owned();
            let mut finfo: pg_sys::FmgrInfo = std::mem::zeroed();
            let (mut infn, mut ioparam) = (pg_sys::InvalidOid, pg_sys::InvalidOid);
            if needed {
                select.push(ch_ident(&name));
                pg_sys::getTypeInputInfo(a.atttypid, &mut infn, &mut ioparam);
                pg_sys::fmgr_info(infn, &mut finfo);
            }
            columns.push(Column {
                finfo,
                ioparam,
                typmod: a.atttypmod,
                target: if !needed {
                    None
                } else {
                    Some(target_type(a.atttypid, a.atttypmod).unwrap_or_else(|| {
                        error!("wildcard_fdw: unsupported PostgreSQL column type or type modifier")
                    }))
                },
            });
        }
        let projection = if select.is_empty() {
            "toUInt8(1) AS __wildcard_row".into()
        } else {
            select.join(", ")
        };
        let sql = format!(
            "SELECT {} FROM {table}{}{}",
            projection,
            str_val(private[0]),
            str_val(private[3]),
        );
        let candidate_where = str_val(private[4]);
        let candidate_sql = (!candidate_where.is_empty()).then(|| {
            format!(
                "SELECT {projection} FROM {table}{candidate_where}{}",
                str_val(private[3])
            )
        });
        let session =
            transport::Session::from_options(&o).unwrap_or_else(|e| error!("wildcard_fdw: {e}"));
        let state = ScanState {
            sql,
            candidate_sql,
            settings: o.get("settings").cloned(),
            lake_summary: str_val(private[1]),
            session,
            stream: None,
            started: false,
            batch: None,
            schema: None,
            batch_row: 0,
            columns,
            rows: 0,
            metrics: transport::StreamMetrics::default(),
            conversion: std::time::Duration::ZERO,
            row_timing: false,
        };
        let state = PgMemoryContexts::CurrentMemoryContext.leak_and_drop_on_delete(state);
        (*node).fdw_state = state as *mut std::ffi::c_void;
        // Dispatch on first tuple demand. LIMIT 0, unused branches and EXPLAIN
        // must not create remote work that then requires draining/confirmation.
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn iterate_foreign_scan(
    node: *mut pg_sys::ForeignScanState,
) -> *mut pg_sys::TupleTableSlot {
    // SAFETY: PostgreSQL invokes this on the backend thread with live scan,
    // slot and expression-context pointers. Arrow storage is borrowed only
    // until its value is copied into the executor's per-tuple memory context.
    unsafe {
        let s = &mut *((*node).fdw_state as *mut ScanState);
        let slot = (*node).ss.ss_ScanTupleSlot;
        let clear = (*(*slot).tts_ops)
            .clear
            .unwrap_or_else(|| error!("wildcard_fdw: missing tuple clear callback"));
        clear(slot);
        if !s.started {
            // ExecInitNode attaches instrumentation after BeginForeignScan.
            // Inspect it here, including after rescans, and retain only a bool.
            // auto_explain or another hook may intentionally enable this too.
            s.row_timing = (*node)
                .ss
                .ps
                .instrument
                .as_ref()
                .is_some_and(|instrument| instrument.need_timer);
            s.stream = Some(
                s.session
                    .start_selected(&s.sql, s.candidate_sql.as_deref(), s.settings.as_deref())
                    .unwrap_or_else(|e| error!("wildcard_fdw: {e}")),
            );
            s.started = true;
        }
        loop {
            if s.batch
                .as_ref()
                .is_some_and(|batch| s.batch_row < batch.num_rows())
            {
                break;
            }
            s.batch = None;
            let Some(stream) = s.stream.as_mut() else {
                return slot;
            };
            match stream
                .receive(|| {
                    pgrx::check_for_interrupts!();
                })
                .unwrap_or_else(|e| error!("wildcard_fdw: {e}"))
            {
                #[cfg(test)]
                transport::Poll::Pending => {
                    continue; // receive waits internally; only nonblocking test polls return Pending.
                }
                transport::Poll::Schema(schema) => {
                    if s.schema.is_some() {
                        error!("wildcard_fdw: duplicate Arrow schema");
                    }
                    let mut targets: Vec<_> =
                        s.columns.iter().filter_map(|col| col.target).collect();
                    if targets.is_empty() {
                        targets.push(values::Target::Int16);
                    }
                    values::validate_schema(&schema, &targets)
                        .unwrap_or_else(|e| error!("wildcard_fdw: {e}"));
                    s.schema = Some(schema);
                }
                transport::Poll::Complete => {
                    if s.schema.is_none() {
                        error!("wildcard_fdw: missing Arrow schema at completion");
                    }
                    s.metrics.accumulate(&stream.metrics());
                    s.stream = None;
                    return slot;
                }
                transport::Poll::Batch(batch) => {
                    if s.schema
                        .as_ref()
                        .is_none_or(|schema| **schema != *batch.schema())
                    {
                        error!(
                            "wildcard_fdw: Arrow schema changed or was not delivered before data"
                        );
                    }
                    s.batch = Some(batch);
                    s.batch_row = 0;
                }
            }
        }
        let batch = s
            .batch
            .as_ref()
            .unwrap_or_else(|| error!("wildcard_fdw: missing Arrow batch"));
        let values = std::slice::from_raw_parts_mut((*slot).tts_values, s.columns.len());
        let nulls = std::slice::from_raw_parts_mut((*slot).tts_isnull, s.columns.len());
        let conversion_start = conversion_timer(s.row_timing, std::time::Instant::now);
        let mut remote = 0;
        // Virtual tuples must not point into a Rust-owned Arrow batch. Allocate
        // all by-reference Datums in the executor context for this output row.
        PgMemoryContexts::For((*(*node).ss.ps.ps_ExprContext).ecxt_per_tuple_memory).switch_to(
            |_| {
                for (i, col) in s.columns.iter_mut().enumerate() {
                    let Some(target) = col.target else {
                        nulls[i] = true;
                        continue;
                    };
                    let input = values::input(batch.column(remote).as_ref(), s.batch_row, target)
                        .unwrap_or_else(|e| error!("wildcard_fdw: {e}"));
                    remote += 1;
                    match input {
                        None => nulls[i] = true,
                        Some(value) => {
                            values[i] =
                                datum::into_datum(value, &mut col.finfo, col.ioparam, col.typmod);
                            nulls[i] = false;
                        }
                    }
                }
            },
        );
        if let Some(start) = conversion_start {
            s.conversion = s.conversion.saturating_add(start.elapsed());
        }
        s.batch_row += 1;
        s.rows += 1;
        pg_sys::ExecStoreVirtualTuple(slot);
        slot
    }
}

fn finish_scan(s: &mut ScanState) {
    // Normal executor completion must not turn an early local LIMIT into
    // success while remote rows are provisional. Error cleanup still
    // aborts through Stream::drop. Release retained bytes before draining.
    s.batch = None;
    if let Some(stream) = s.stream.as_mut() {
        loop {
            match stream
                .receive(|| {
                    pgrx::check_for_interrupts!();
                })
                .unwrap_or_else(|e| error!("wildcard_fdw: {e}"))
            {
                transport::Poll::Schema(schema) => {
                    if s.schema.is_some() {
                        error!("wildcard_fdw: duplicate Arrow schema");
                    }
                    let mut targets: Vec<_> = s.columns.iter().filter_map(|c| c.target).collect();
                    if targets.is_empty() {
                        targets.push(values::Target::Int16);
                    }
                    values::validate_schema(&schema, &targets)
                        .unwrap_or_else(|e| error!("wildcard_fdw: {e}"));
                    s.schema = Some(schema);
                }
                transport::Poll::Batch(batch) => {
                    if s.schema
                        .as_ref()
                        .is_none_or(|schema| **schema != *batch.schema())
                    {
                        error!("wildcard_fdw: Arrow schema changed or missing");
                    }
                    drop(batch);
                }
                transport::Poll::Complete => {
                    if s.schema.is_none() {
                        error!("wildcard_fdw: missing Arrow schema at completion");
                    }
                    s.metrics.accumulate(&stream.metrics());
                    break;
                }
                #[cfg(test)]
                transport::Poll::Pending => continue,
            }
        }
    }
    s.stream = None;
    s.batch = None;
}

#[pg_guard]
unsafe extern "C-unwind" fn rescan_foreign_scan(node: *mut pg_sys::ForeignScanState) {
    unsafe {
        let s = &mut *((*node).fdw_state as *mut ScanState);
        finish_scan(s);
        s.schema = None;
        s.batch_row = 0;
        s.rows = 0;
        s.started = false;
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn end_foreign_scan(node: *mut pg_sys::ForeignScanState) {
    unsafe {
        if let Some(s) = ((*node).fdw_state as *mut ScanState).as_mut() {
            finish_scan(s);
        }
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn explain_foreign_scan(
    node: *mut pg_sys::ForeignScanState,
    es: *mut pg_sys::ExplainState,
) {
    unsafe {
        let s = &*((*node).fdw_state as *mut ScanState);
        let label = c"ClickHouse SQL";
        let mut observed = s.metrics.clone();
        if let Some(stream) = &s.stream {
            observed.accumulate(&stream.metrics());
        }
        let selected_sql = if observed.preparation.selected {
            s.candidate_sql.as_deref().unwrap_or(&s.sql)
        } else {
            &s.sql
        };
        let value = CString::new(selected_sql.replace('\0', ""))
            .unwrap_or_else(|_| error!("wildcard_fdw: invalid explain SQL"));
        pg_sys::ExplainPropertyText(label.as_ptr(), value.as_ptr(), es);
        if let Some(candidate) = &s.candidate_sql {
            let value = CString::new(candidate.replace('\0', ""))
                .unwrap_or_else(|_| error!("wildcard_fdw: invalid candidate SQL"));
            pg_sys::ExplainPropertyText(c"ClickHouse candidate SQL".as_ptr(), value.as_ptr(), es);
        }
        if (*es).analyze {
            let m = observed;
            let reason = CString::new(if m.preparation.reason.is_empty() {
                "not_requested"
            } else {
                m.preparation.reason
            })
            .unwrap_or_default();
            pg_sys::ExplainPropertyText(
                c"FDW automatic candidate decision".as_ptr(),
                reason.as_ptr(),
                es,
            );
            pg_sys::ExplainPropertyBool(
                c"FDW automatic candidate selected".as_ptr(),
                m.preparation.selected,
                es,
            );
            // PostgreSQL prints EXPLAIN before EndForeignScan. An early local
            // stop may still need draining/confirmation; never label its current
            // counters as a complete measurement.
            pg_sys::ExplainPropertyBool(c"FDW metrics complete".as_ptr(), s.stream.is_none(), es);
            pg_sys::ExplainPropertyBool(c"FDW row timing enabled".as_ptr(), s.row_timing, es);
            for (name, value) in [
                (
                    c"FDW plan preparation",
                    m.preparation.elapsed.as_secs_f64() * 1000.0,
                ),
                (
                    c"FDW plan transfer wait",
                    m.preparation.transfer.as_secs_f64() * 1000.0,
                ),
                (
                    c"FDW plan decode",
                    m.preparation.decode.as_secs_f64() * 1000.0,
                ),
                (
                    c"FDW plan framing decode",
                    m.preparation.framing.as_secs_f64() * 1000.0,
                ),
                (
                    c"FDW plan terminal",
                    m.preparation.terminal.as_secs_f64() * 1000.0,
                ),
                (c"FDW consumer wait", m.consumer_wait.as_secs_f64() * 1000.0),
                (
                    c"FDW producer queue wait",
                    m.producer_send_wait.as_secs_f64() * 1000.0,
                ),
                (c"FDW row conversion", s.conversion.as_secs_f64() * 1000.0),
            ] {
                pg_sys::ExplainPropertyFloat(name.as_ptr(), c"ms".as_ptr(), value, 3, es);
            }
            for (name, value) in [
                (c"FDW plan HTTP body bytes", m.preparation.body_bytes),
                (c"FDW plan control requests", m.preparation.controls),
                (c"FDW plan probes", m.preparation.probe_queries),
                (c"FDW plan cache hits", m.preparation.cache_hits),
                (c"FDW batches", m.batches),
                (c"FDW remote rows", m.rows),
                (c"FDW charged batch bytes", m.charged_bytes),
                (c"FDW wait wakeups", m.consumer_waits),
            ] {
                pg_sys::ExplainPropertyUInteger(name.as_ptr(), std::ptr::null(), value, es);
            }
            if let Some(e) = m.execution {
                for (name, value) in [
                    (c"FDW HTTP body wait", e.transfer),
                    (c"FDW Arrow decode", e.decode),
                    (c"FDW framing decode", Some(e.framing)),
                ] {
                    if let Some(value) = value {
                        pg_sys::ExplainPropertyFloat(
                            name.as_ptr(),
                            c"ms".as_ptr(),
                            value.as_secs_f64() * 1000.0,
                            3,
                            es,
                        );
                    }
                }
                pg_sys::ExplainPropertyUInteger(
                    c"FDW HTTP body bytes".as_ptr(),
                    std::ptr::null(),
                    e.wire_bytes,
                    es,
                );
                pg_sys::ExplainPropertyUInteger(
                    c"FDW data control requests".as_ptr(),
                    std::ptr::null(),
                    e.control_requests,
                    es,
                );
                pg_sys::ExplainPropertyFloat(
                    c"FDW terminal confirmation".as_ptr(),
                    c"ms".as_ptr(),
                    e.terminal.as_secs_f64() * 1000.0,
                    3,
                    es,
                );
                pg_sys::ExplainPropertyFloat(
                    c"FDW upstream data polling".as_ptr(),
                    c"ms".as_ptr(),
                    e.data_poll.as_secs_f64() * 1000.0,
                    3,
                    es,
                );
            }
        }
        if !s.lake_summary.is_empty() {
            let label = c"Lake files";
            let value = CString::new(s.lake_summary.as_str())
                .unwrap_or_else(|_| error!("wildcard_fdw: invalid explain summary"));
            pg_sys::ExplainPropertyText(label.as_ptr(), value.as_ptr(), es);
        }
    }
}

// ------------------------------------------------------------------ handler

#[pg_extern(sql = r#"
CREATE FUNCTION wildcard_fdw_handler() RETURNS fdw_handler
    STRICT LANGUAGE c AS 'MODULE_PATHNAME', '@FUNCTION_NAME@';
CREATE FOREIGN DATA WRAPPER wildcard_fdw HANDLER wildcard_fdw_handler;
"#)]
fn wildcard_fdw_handler() -> PgBox<pg_sys::FdwRoutine> {
    // SAFETY: allocates a zeroed FdwRoutine node in the current memory context.
    let mut r = unsafe { PgBox::<pg_sys::FdwRoutine>::alloc_node(pg_sys::NodeTag::T_FdwRoutine) };
    r.GetForeignRelSize = Some(get_foreign_rel_size);
    r.GetForeignPaths = Some(get_foreign_paths);
    r.GetForeignPlan = Some(get_foreign_plan);
    r.BeginForeignScan = Some(begin_foreign_scan);
    r.IterateForeignScan = Some(iterate_foreign_scan);
    r.ReScanForeignScan = Some(rescan_foreign_scan);
    r.EndForeignScan = Some(end_foreign_scan);
    r.ExplainForeignScan = Some(explain_foreign_scan);
    r.into_pg_boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_timer_is_lazy_when_executor_timing_is_disabled() {
        let disabled: Option<()> = conversion_timer(false, || panic!("unexpected clock read"));
        assert!(disabled.is_none());
        let reads = std::cell::Cell::new(0);
        let enabled = conversion_timer(true, || {
            reads.set(reads.get() + 1);
            42
        });
        assert_eq!(enabled, Some(42));
        assert_eq!(reads.get(), 1);
    }

    #[test]
    fn idents() {
        assert_eq!(ch_ident("url"), "`url`");
        assert_eq!(ch_ident("we`ird"), "`we\\`ird`");
    }
}
