// SPDX-License-Identifier: Apache-2.0
//! Conservative projection and ordered page reduction; all quals stay local.
use super::*;

/// Physical attribute numbers, including dependencies used only by local quals.
pub(super) unsafe fn attributes(
    tlist: *mut pg_sys::List,
    local: *mut pg_sys::List,
    scanrelid: u32,
    relid: pg_sys::Oid,
) -> String {
    // SAFETY: planner-owned expression trees and relation descriptor are alive.
    unsafe {
        let mut used = std::ptr::null_mut();
        pg_sys::pull_varattnos(tlist.cast(), scanrelid, &mut used);
        pg_sys::pull_varattnos(local.cast(), scanrelid, &mut used);
        let shift = -pg_sys::FirstLowInvalidHeapAttributeNumber;
        let whole = pg_sys::bms_is_member(shift, used);
        let rel = pg_sys::table_open(relid, pg_sys::NoLock as i32);
        let desc = pgrx::PgTupleDesc::from_pg_unchecked((*rel).rd_att);
        let attrs = desc
            .iter()
            .enumerate()
            .filter_map(|(i, a)| {
                let n = i as i32 + 1;
                (!a.attisdropped && (whole || pg_sys::bms_is_member(n + shift, used)))
                    .then(|| n.to_string())
            })
            .collect::<Vec<_>>()
            .join(",");
        pg_sys::bms_free(used);
        pg_sys::table_close(rel, pg_sys::NoLock as i32);
        attrs
    }
}

unsafe fn bound(n: *mut pg_sys::Node) -> Option<u64> {
    // SAFETY: const_text validates the node kind before inspecting a Const.
    unsafe {
        let (text, ty) = const_text(n)?;
        (ty == pg_sys::INT8OID).then_some(())?;
        text.parse().ok()
    }
}

/// Stronger than deparsing: a limit must never precede an inexact local filter.
unsafe fn exact_qual(
    n: *mut pg_sys::Node,
    relid: pg_sys::Oid,
    scanrelid: u32,
    materialized: bool,
) -> bool {
    // SAFETY: all nodes are PostgreSQL-owned planner expressions.
    unsafe {
        if n.is_null() || deparse(n, relid, scanrelid, materialized).is_none() {
            return false;
        }
        match (*n).type_ {
            pg_sys::NodeTag::T_BoolExpr => {
                PgList::<pg_sys::Node>::from_pg((*(n as *mut pg_sys::BoolExpr)).args)
                    .iter_ptr()
                    .all(|p| exact_qual(p, relid, scanrelid, materialized))
            }
            pg_sys::NodeTag::T_FuncExpr => {
                let f = &*(n as *mut pg_sys::FuncExpr);
                let ext = pg_sys::get_extension_oid(c"pg_wildcard".as_ptr(), true);
                ext != pg_sys::InvalidOid
                    && pg_sys::getExtensionOfObject(pg_sys::ProcedureRelationId, f.funcid) == ext
                    && !pg_sys::contain_volatile_functions(n)
            }
            pg_sys::NodeTag::T_OpExpr => {
                let op = &*(n as *mut pg_sys::OpExpr);
                let builtin = pg_sys::get_func_namespace(pg_sys::get_opcode(op.opno))
                    == pg_sys::Oid::from(pg_sys::PG_CATALOG_NAMESPACE);
                // Nondefault collations and floating/temporal comparisons need their
                // own parity qualification before they may justify a remote limit.
                let args = PgList::<pg_sys::Node>::from_pg(op.args);
                let first = args.iter_ptr().next().map(|p| strip_relabel(p));
                let rhs = args
                    .iter_ptr()
                    .nth(1)
                    .and_then(|p| const_text(p))
                    .map(|(_, ty)| ty);
                builtin
                    && first.is_some_and(|p| {
                        if (*p).type_ != pg_sys::NodeTag::T_Var {
                            return false;
                        }
                        let ty = (*(p as *mut pg_sys::Var)).vartype;
                        (matches!(ty, pg_sys::INT2OID | pg_sys::INT4OID | pg_sys::INT8OID)
                            && rhs.is_some_and(|t| {
                                matches!(t, pg_sys::INT2OID | pg_sys::INT4OID | pg_sys::INT8OID)
                            }))
                            || (ty == pg_sys::BOOLOID && rhs == Some(pg_sys::BOOLOID))
                            || (matches!(ty, pg_sys::TEXTOID | pg_sys::VARCHAROID)
                                && matches!(
                                    CStr::from_ptr(pg_sys::get_opname(op.opno)).to_bytes(),
                                    b"~~" | b"!~~"
                                )
                                && (op.inputcollid == pg_sys::InvalidOid
                                    || byte_collation(op.inputcollid)))
                    })
            }
            pg_sys::NodeTag::T_NullTest => !(*(n as *mut pg_sys::NullTest)).argisrow,
            _ => false,
        }
    }
}

/// Fetch the first limit+offset rows remotely; PostgreSQL keeps its own Sort/Limit.
/// No remote OFFSET: it would discard rows twice. Unsupported shapes scan fully.
pub(super) unsafe fn ordered_limit(
    root: *mut pg_sys::PlannerInfo,
    baserel: *mut pg_sys::RelOptInfo,
    relid: pg_sys::Oid,
    clauses: *mut pg_sys::List,
    materialized: bool,
) -> Option<String> {
    // SAFETY: callback receives live planner/query nodes; each cast checks its tag.
    unsafe {
        let q = &*(*root).parse;
        if !(*root).parent_root.is_null()
            || q.commandType != pg_sys::CmdType::CMD_SELECT
            || q.hasAggs
            || q.hasWindowFuncs
            || q.hasTargetSRFs
            || q.hasSubLinks
            || q.hasRowSecurity
            || q.hasForUpdate
            || !q.groupClause.is_null()
            || !q.groupingSets.is_null()
            || !q.havingQual.is_null()
            || !q.distinctClause.is_null()
            || !q.setOperations.is_null()
            || !q.cteList.is_null()
            || !q.rowMarks.is_null()
            || q.sortClause.is_null()
            || q.limitOption != pg_sys::LimitOption::LIMIT_OPTION_COUNT
            || q.jointree.is_null()
            || pg_sys::contain_volatile_functions(q.targetList.cast())
        {
            return None;
        }
        let from = PgList::<pg_sys::Node>::from_pg((*q.jointree).fromlist);
        if from.len() != 1 {
            return None;
        }
        let source = from.iter_ptr().next()?;
        if (*source).type_ != pg_sys::NodeTag::T_RangeTblRef
            || (*(source as *mut pg_sys::RangeTblRef)).rtindex as u32 != (*baserel).relid
        {
            return None;
        }
        let rte = PgList::<pg_sys::RangeTblEntry>::from_pg(q.rtable)
            .iter_ptr()
            .nth((*baserel).relid as usize - 1)?;
        if (*rte).inh || !(*rte).securityQuals.is_null() {
            return None;
        }
        let limit = bound(q.limitCount)?;
        let offset = if q.limitOffset.is_null() {
            0
        } else {
            bound(q.limitOffset)?
        };
        let fetch = limit.checked_add(offset)?;
        if fetch > 1_000_000 {
            return None;
        }
        let local = pg_sys::extract_actual_clauses(clauses, false);
        if !PgList::<pg_sys::Node>::from_pg(local)
            .iter_ptr()
            .all(|p| exact_qual(p, relid, (*baserel).relid, materialized))
        {
            return None;
        }
        let mut order = Vec::new();
        for c in PgList::<pg_sys::SortGroupClause>::from_pg(q.sortClause).iter_ptr() {
            let expr = strip_relabel(pg_sys::get_sortgroupclause_expr(c, q.targetList));
            let column = var_column(expr, relid, (*baserel).relid)?;
            let v = &*(expr as *mut pg_sys::Var);
            if !matches!(
                v.vartype,
                pg_sys::INT2OID | pg_sys::INT4OID | pg_sys::INT8OID
            ) {
                return None;
            }
            if (*c).sortop.to_u32() >= pg_sys::FirstNormalObjectId
                || pg_sys::get_func_namespace(pg_sys::get_opcode((*c).sortop))
                    != pg_sys::Oid::from(pg_sys::PG_CATALOG_NAMESPACE)
            {
                return None;
            }
            let op = CStr::from_ptr(pg_sys::get_opname((*c).sortop)).to_bytes();
            let direction = match op {
                b"<" => "ASC",
                b">" => "DESC",
                _ => return None,
            };
            let nulls = if (*c).nulls_first { "FIRST" } else { "LAST" };
            order.push(format!("{} {direction} NULLS {nulls}", ch_ident(&column)));
        }
        Some(format!(" ORDER BY {} LIMIT {fetch}", order.join(", ")))
    }
}

pub(super) unsafe fn byte_collation(oid: pg_sys::Oid) -> bool {
    if oid == pg_sys::C_COLLATION_OID {
        return true;
    }
    #[cfg(feature = "pg18")]
    // SAFETY: PostgreSQL owns and caches the locale for this planner collation.
    unsafe {
        (*pg_sys::pg_newlocale_from_collation(oid)).collate_is_c
    }
    #[cfg(not(feature = "pg18"))]
    false
}
