// SPDX-License-Identifier: Apache-2.0
//! Planner support function: rewrite `wc_*` calls into expressions that
//! pg_clickhouse (and Postgres indexes) already understand.
//!
//! - `wc_match(col, 'p', 'raw')` / `wc_like(col, 'p', 'raw')` → `col LIKE 'p'`
//!   (any table: same semantics for valid UTF-8 text, and it lets btree or
//!   pg_trgm indexes help locally).
//! - Non-raw views keep their original calls: a similarly named foreign
//!   column does not prove that it contains the requested transformation.
//! - `wc_regex(col, 'p', ci, 'raw')` → `re2match(col, '[(?i)]p')` only for
//!   the member of the installed `re2` extension (the pg_re2 project).
//!
//! Anything else is left unchanged and evaluated by the `wc_*` function.
//! Our own FDW keeps extension predicates intact: their qualified semantics
//! justify exact pushdown/top-K without relying on a database LIKE collation.

use std::ffi::{CStr, CString};

use pgrx::pg_sys;
use pgrx::prelude::*;
use pgrx::{Internal, PgList};
use wildcard_core::View;
use wildcard_core::verify::{self, Kind};

const TEXT_LIKE_OP: u32 = 1209; // ~~(text, text)

unsafe fn strip_relabel(mut n: *mut pg_sys::Node) -> *mut pg_sys::Node {
    unsafe {
        while !n.is_null() && (*n).type_ == pg_sys::NodeTag::T_RelabelType {
            n = (*(n as *mut pg_sys::RelabelType)).arg as *mut pg_sys::Node;
        }
        n
    }
}

unsafe fn const_str(n: *mut pg_sys::Node) -> Option<String> {
    unsafe {
        let n = strip_relabel(n);
        if n.is_null()
            || (*n).type_ != pg_sys::NodeTag::T_Const
            || (*(n as *mut pg_sys::Const)).constisnull
        {
            return None;
        }
        let c = n as *mut pg_sys::Const;
        let mut out_fn = pg_sys::InvalidOid;
        let mut varlena = false;
        pg_sys::getTypeOutputInfo((*c).consttype, &mut out_fn, &mut varlena);
        Some(
            CStr::from_ptr(pg_sys::OidOutputFunctionCall(out_fn, (*c).constvalue))
                .to_string_lossy()
                .into_owned(),
        )
    }
}

unsafe fn text_const(s: &str) -> *mut pg_sys::Node {
    unsafe {
        let c = CString::new(s.replace('\0', "")).unwrap();
        let datum = pg_sys::Datum::from(pg_sys::cstring_to_text(c.as_ptr()));
        pg_sys::makeConst(
            pg_sys::TEXTOID,
            -1,
            pg_sys::DEFAULT_COLLATION_OID,
            -1,
            datum,
            false,
            false,
        ) as *mut pg_sys::Node
    }
}

/// Resolve the actual re2 extension member, independently of search_path.
unsafe fn re2match_oid() -> pg_sys::Oid {
    // SAFETY: all catalog access stays on the backend. The syscache tuple is
    // live until released; copy its namespace before releasing the reference.
    unsafe {
        let extension = pg_sys::get_extension_oid(c"re2".as_ptr(), true);
        if extension == pg_sys::InvalidOid {
            return pg_sys::InvalidOid;
        }
        // PostgreSQL 17 names the extension OID cache differently.
        #[cfg(feature = "pg17")]
        let cache = pg_sys::SysCacheIdentifier::ZEXTENSIONOID;
        #[cfg(not(feature = "pg17"))]
        let cache = pg_sys::SysCacheIdentifier::EXTENSIONOID;
        let tuple = pg_sys::SearchSysCache1(cache as i32, pg_sys::Datum::from(extension));
        if tuple.is_null() {
            return pg_sys::InvalidOid;
        }
        let namespace =
            (*pg_sys::GETSTRUCT(tuple).cast::<pg_sys::FormData_pg_extension>()).extnamespace;
        pg_sys::ReleaseSysCache(tuple);
        let schema = pg_sys::get_namespace_name(namespace);
        if schema.is_null() {
            return pg_sys::InvalidOid;
        }
        let mut names = PgList::<pg_sys::Node>::new();
        names.push(pg_sys::makeString(schema) as *mut pg_sys::Node);
        names.push(pg_sys::makeString(pg_sys::pstrdup(c"re2match".as_ptr())) as *mut pg_sys::Node);
        let mut argtypes = [pg_sys::TEXTOID, pg_sys::TEXTOID];
        let oid = pg_sys::LookupFuncName(names.into_pg(), 2, argtypes.as_mut_ptr(), true);
        if oid != pg_sys::InvalidOid
            && pg_sys::getExtensionOfObject(pg_sys::ProcedureRelationId, oid) == extension
            && pg_sys::get_func_rettype(oid) == pg_sys::BOOLOID
            && !pg_sys::get_func_retset(oid)
        {
            oid
        } else {
            pg_sys::InvalidOid
        }
    }
}

/// Identify our handler by extension membership, including wrapper aliases.
unsafe fn our_fdw_var(
    root: *mut pg_sys::PlannerInfo,
    arg: *mut pg_sys::Node,
    extension: pg_sys::Oid,
) -> bool {
    // SAFETY: the support callback owns live planner nodes; catalogs own the
    // relation/server/handler identities. Simplification can precede rel arrays.
    unsafe {
        let arg = strip_relabel(arg);
        if root.is_null()
            || (*root).parse.is_null()
            || arg.is_null()
            || (*arg).type_ != pg_sys::NodeTag::T_Var
        {
            return false;
        }
        let v = &*(arg as *mut pg_sys::Var);
        if v.varlevelsup != 0 || v.varattno <= 0 || v.varno == 0 {
            return false;
        }
        let Some(rte) = PgList::<pg_sys::RangeTblEntry>::from_pg((*(*root).parse).rtable)
            .iter_ptr()
            .nth(v.varno as usize - 1)
        else {
            return false;
        };
        if rte.is_null()
            || (*rte).rtekind != pg_sys::RTEKind::RTE_RELATION
            || (*rte).relkind != pg_sys::RELKIND_FOREIGN_TABLE as std::ffi::c_char
        {
            return false;
        }
        let table = pg_sys::GetForeignTable((*rte).relid);
        let server = pg_sys::GetForeignServer((*table).serverid);
        let wrapper = pg_sys::GetForeignDataWrapper((*server).fdwid);
        let handler = (*wrapper).fdwhandler;
        handler != pg_sys::InvalidOid
            && pg_sys::getExtensionOfObject(pg_sys::ProcedureRelationId, handler) == extension
            && CStr::from_ptr(pg_sys::get_func_name(handler)).to_bytes() == b"wildcard_fdw_handler"
    }
}

unsafe fn simplify(req: *mut pg_sys::SupportRequestSimplify) -> Option<*mut pg_sys::Node> {
    unsafe {
        let f = (*req).fcall;
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
        let (kind, view_idx) = match (name.as_str(), args.len()) {
            ("wc_match", 3) => (Kind::Wildcard, 2),
            ("wc_like", 3) => (Kind::Like, 2),
            ("wc_regex", 4) => (
                Kind::Regex {
                    case_insensitive: const_str(args[2])? == "t",
                },
                3,
            ),
            _ => return None,
        };
        if our_fdw_var((*req).root, args[0], extension) {
            return None;
        }
        let pattern = const_str(args[1])?;
        let view: View = const_str(args[view_idx])?.parse().ok()?;
        if view != View::Raw {
            // Column names alone cannot establish materialized-view identity.
            // Keep the exact transformation and recheck on the original value.
            return None;
        }
        let target = args[0];
        match kind {
            Kind::Wildcard | Kind::Like => {
                let pieces = verify::pieces_for_checked(kind, pattern.as_bytes(), view).ok()?;
                let like = wildcard_ch::like_pattern(&pieces);
                let op = pg_sys::make_opclause(
                    pg_sys::Oid::from(TEXT_LIKE_OP),
                    pg_sys::BOOLOID,
                    false,
                    target as *mut pg_sys::Expr,
                    text_const(&like) as *mut pg_sys::Expr,
                    pg_sys::InvalidOid,
                    pg_sys::DEFAULT_COLLATION_OID,
                );
                Some(op as *mut pg_sys::Node)
            }
            Kind::Regex { case_insensitive } => {
                wildcard_core::re2::to_rust(&pattern).ok()?;
                let oid = re2match_oid();
                if oid == pg_sys::InvalidOid {
                    return None;
                }
                let p = if case_insensitive {
                    format!("(?i){pattern}")
                } else {
                    pattern
                };
                let mut a = PgList::<pg_sys::Node>::new();
                a.push(target);
                a.push(text_const(&p));
                Some(pg_sys::makeFuncExpr(
                    oid,
                    pg_sys::BOOLOID,
                    a.into_pg(),
                    pg_sys::InvalidOid,
                    pg_sys::DEFAULT_COLLATION_OID,
                    pg_sys::CoercionForm::COERCE_EXPLICIT_CALL,
                ) as *mut pg_sys::Node)
            }
        }
    }
}

/// "No simplification": a NULL *pointer* (not SQL NULL, which Postgres rejects).
fn no_rewrite() -> Internal {
    Internal::from(Some(pg_sys::Datum::from(0usize)))
}

/// Support function attached to `wc_match`, `wc_like` and `wc_regex`.
#[pg_extern(immutable, parallel_safe)]
fn wc_rewrite_support(req: Internal) -> Internal {
    // SAFETY: Postgres passes a planner support request node.
    unsafe {
        let Some(datum) = req.unwrap() else {
            return no_rewrite();
        };
        let node = datum.cast_mut_ptr::<pg_sys::Node>();
        if node.is_null() || (*node).type_ != pg_sys::NodeTag::T_SupportRequestSimplify {
            return no_rewrite();
        }
        match simplify(node as *mut pg_sys::SupportRequestSimplify) {
            Some(n) => Internal::from(Some(pg_sys::Datum::from(n))),
            None => no_rewrite(),
        }
    }
}

extension_sql!(
    r#"
ALTER FUNCTION wc_match(text, text, text) SUPPORT wc_rewrite_support;
ALTER FUNCTION wc_like(text, text, text) SUPPORT wc_rewrite_support;
ALTER FUNCTION wc_regex(text, text, boolean, text) SUPPORT wc_rewrite_support;
"#,
    name = "wc_support",
    finalize
);
