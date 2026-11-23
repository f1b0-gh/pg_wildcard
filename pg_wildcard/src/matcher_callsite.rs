// SPDX-License-Identifier: Apache-2.0
//! Executor-local constant hints; the bounded backend cache owns the matchers.

use std::marker::PhantomData;
use std::rc::{Rc, Weak};

use libwildcard::core::{Matcher, verify::Kind};
use pgrx::callconv::{Arg, ArgAbi};
use pgrx::pgrx_sql_entity_graph::metadata::{
    ArgumentError, ReturnsError, ReturnsRef, SqlMappingRef, SqlTranslatable, TypeOrigin,
};
use pgrx::{FromDatum, PgList, pg_sys};

/// A SQL text datum borrowed for this function invocation. Defer decoding so
/// constant pattern/view arguments need no detoasting on a call-site hit.
pub(crate) struct TextDatum<'fcx> {
    datum: pg_sys::Datum,
    _call: PhantomData<&'fcx ()>,
}

// SAFETY: the generated SQL signature is text; unboxing only retains the datum
// for the invocation lifetime supplied by pgrx. Strict functions reject NULL.
unsafe impl<'fcx> ArgAbi<'fcx> for TextDatum<'fcx> {
    unsafe fn unbox_arg_unchecked(arg: Arg<'_, 'fcx>) -> Self {
        // SAFETY: ArgAbi requires a non-null datum of the declared SQL type.
        let datum = unsafe { arg.unbox_arg_using_from_datum::<pg_sys::Datum>() }
            .expect("strict text argument must not be null");
        Self {
            datum,
            _call: PhantomData,
        }
    }
}

// SAFETY: TextDatum is only an argument representation of PostgreSQL text.
// Delegate SQL metadata to the existing text mapping, preserving the API.
unsafe impl SqlTranslatable for TextDatum<'_> {
    const TYPE_IDENT: &'static str = <&str as SqlTranslatable>::TYPE_IDENT;
    const TYPE_ORIGIN: TypeOrigin = <&str as SqlTranslatable>::TYPE_ORIGIN;
    const ARGUMENT_SQL: Result<SqlMappingRef, ArgumentError> =
        <&str as SqlTranslatable>::ARGUMENT_SQL;
    const RETURN_SQL: Result<ReturnsRef, ReturnsError> = <&str as SqlTranslatable>::RETURN_SQL;
}

impl TextDatum<'_> {
    fn text(&self) -> &str {
        // SAFETY: datum is a non-null text argument still alive in this call.
        // pgrx preserves validation for SQL_ASCII and other server encodings.
        unsafe { <&str>::from_datum(self.datum, false) }.expect("non-null text")
    }

    fn bytes(&self, utf8_database: bool) -> &[u8] {
        if !utf8_database {
            return self.text().as_bytes();
        }
        // SAFETY: PostgreSQL validates text in a UTF8 database. Ordinary and
        // short varlenas can be read directly; external/expanded/compressed
        // forms must use pgrx's guarded PostgreSQL detoasting binding first.
        // Both the original and detoasted allocation live through this call.
        unsafe {
            let mut value = self.datum.cast_mut_ptr::<pg_sys::varlena>();
            if pgrx::varlena::varatt_is_1b_e(value) || pgrx::varlena::varatt_is_4b_c(value) {
                value = pg_sys::pg_detoast_datum_packed(value);
            }
            pgrx::varlena::varlena_to_byte_slice(value)
        }
    }
}

struct CallSite {
    constants: bool,
    utf8_database: bool,
    // A strong reference here would let every prepared plan retain a matcher
    // after eviction. Weak hints expire when the backend cache releases it.
    matcher: Weak<Matcher>,
}

#[repr(C)]
struct ContextSlot {
    callback: pg_sys::MemoryContextCallback,
    site: CallSite,
}

#[pgrx::pg_guard]
unsafe extern "C-unwind" fn drop_site(arg: *mut std::ffi::c_void) {
    // SAFETY: the callback is registered once on the fn_mcxt allocation and is
    // invoked before that allocation is freed, on either reset or deletion.
    unsafe { std::ptr::drop_in_place(arg.cast::<CallSite>()) };
}

/// Accept only actual Const nodes, never PARAM_EXTERN (which PostgreSQL's
/// get_fn_expr_arg_stable also accepts) or expressions evaluated on a rescan.
unsafe fn constant_arguments(flinfo: *const pg_sys::FmgrInfo) -> bool {
    // SAFETY: fn_expr is PostgreSQL's expression tree, owned by this FmgrInfo.
    unsafe {
        let expr = (*flinfo).fn_expr;
        if expr.is_null() || (*expr).type_ != pg_sys::NodeTag::T_FuncExpr {
            return false;
        }
        let args = PgList::<pg_sys::Node>::from_pg((*expr.cast::<pg_sys::FuncExpr>()).args);
        args.len() >= 3
            && args.iter_ptr().skip(1).all(|arg| {
                !arg.is_null()
                    && (*arg).type_ == pg_sys::NodeTag::T_Const
                    && !(*arg.cast::<pg_sys::Const>()).constisnull
            })
    }
}

unsafe fn callsite<'a>(fcinfo: pg_sys::FunctionCallInfo) -> &'a mut CallSite {
    // SAFETY: this is only called by our generated pgrx wrappers, which own
    // fn_extra exclusively. Allocate the callback and state together before
    // creating Rust-owned state. All PostgreSQL calls use guarded bindings;
    // allocation failure cannot leak a Box or jump across Rust frames.
    unsafe {
        let flinfo = (*fcinfo).flinfo;
        if (*flinfo).fn_extra.is_null() {
            let constants = constant_arguments(flinfo);
            let utf8_database = pg_sys::GetDatabaseEncoding() == pg_sys::pg_enc::PG_UTF8 as i32;
            let slot =
                pg_sys::MemoryContextAlloc((*flinfo).fn_mcxt, std::mem::size_of::<ContextSlot>())
                    .cast::<ContextSlot>();
            std::ptr::addr_of_mut!((*slot).site).write(CallSite {
                constants,
                utf8_database,
                matcher: Weak::new(),
            });
            std::ptr::addr_of_mut!((*slot).callback).write(pg_sys::MemoryContextCallback {
                func: Some(drop_site),
                arg: std::ptr::addr_of_mut!((*slot).site).cast(),
                next: std::ptr::null_mut(),
            });
            pg_sys::MemoryContextRegisterResetCallback(
                (*flinfo).fn_mcxt,
                std::ptr::addr_of_mut!((*slot).callback),
            );
            (*flinfo).fn_extra = slot.cast();
        }
        &mut (*(*flinfo).fn_extra.cast::<ContextSlot>()).site
    }
}

/// # Safety
/// fcinfo must be the live invocation corresponding to these arguments.
pub(crate) unsafe fn is_match(
    fcinfo: pg_sys::FunctionCallInfo,
    value: TextDatum<'_>,
    pattern: TextDatum<'_>,
    kind: Kind,
    view: TextDatum<'_>,
) -> bool {
    // SAFETY: PostgreSQL direct-function callers may omit FmgrInfo entirely.
    // They have no expression/context in which a hint can be retained.
    if unsafe { (*fcinfo).flinfo.is_null() } {
        return crate::matcher(pattern.text(), kind, crate::view_arg(view.text()))
            .is_match(value.text().as_bytes());
    }
    // SAFETY: the generated wrapper supplies its live fcinfo; no reference to
    // its state or argument bytes escapes this invocation.
    let site = unsafe { callsite(fcinfo) };
    let cached = if site.constants {
        crate::CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            crate::matcher_config::configure(&mut cache);
            cache.get_callsite(&site.matcher)
        })
    } else {
        None
    };
    let matcher = cached.unwrap_or_else(|| {
        let matcher = crate::matcher(pattern.text(), kind, crate::view_arg(view.text()));
        if site.constants {
            site.matcher = Rc::downgrade(&matcher);
        }
        matcher
    });
    matcher.is_match(value.bytes(site.utf8_database))
}

#[cfg(any(test, feature = "pg_test"))]
#[pgrx::pg_schema]
mod tests {
    use super::*;
    use libwildcard::core::views::View;
    use pgrx::prelude::*;

    #[pg_test]
    fn callsite_specializes_only_literal_constants() {
        // SAFETY: all expression nodes and list members stay alive until the
        // classification finishes. No executor evaluates these test nodes.
        unsafe {
            let mut value: pg_sys::Var = std::mem::zeroed();
            value.xpr.type_ = pg_sys::NodeTag::T_Var;
            let mut pattern: pg_sys::Const = std::mem::zeroed();
            pattern.xpr.type_ = pg_sys::NodeTag::T_Const;
            let mut view: pg_sys::Const = std::mem::zeroed();
            view.xpr.type_ = pg_sys::NodeTag::T_Const;
            let mut parameter: pg_sys::Param = std::mem::zeroed();
            parameter.xpr.type_ = pg_sys::NodeTag::T_Param;
            parameter.paramkind = pg_sys::ParamKind::PARAM_EXTERN;
            for (pattern_node, expected) in [
                (std::ptr::addr_of_mut!(pattern).cast::<pg_sys::Node>(), true),
                (std::ptr::addr_of_mut!(parameter).cast(), false),
                (std::ptr::addr_of_mut!(value).cast(), false),
            ] {
                let mut args = PgList::<pg_sys::Node>::new();
                args.push(std::ptr::addr_of_mut!(value).cast());
                args.push(pattern_node);
                args.push(std::ptr::addr_of_mut!(view).cast());
                let mut expr: pg_sys::FuncExpr = std::mem::zeroed();
                expr.xpr.type_ = pg_sys::NodeTag::T_FuncExpr;
                expr.args = args.as_ptr();
                let mut flinfo: pg_sys::FmgrInfo = std::mem::zeroed();
                flinfo.fn_expr = std::ptr::addr_of_mut!(expr).cast();
                assert_eq!(constant_arguments(&flinfo), expected);
            }
        }
    }

    #[pg_test]
    fn callsite_context_reset_and_delete_release_weak_references() {
        let matcher = Rc::new(Matcher::new(Kind::Wildcard, "context*", View::Raw).unwrap());
        let mut context = pgrx::PgMemoryContexts::new("wildcard callsite reset test");
        // SAFETY: zero-initialized fmgr structures are used only by callsite(),
        // with the live context explicitly supplied. No argument is read.
        unsafe {
            let mut flinfo: pg_sys::FmgrInfo = std::mem::zeroed();
            flinfo.fn_mcxt = context.value();
            let mut fcinfo: pg_sys::FunctionCallInfoBaseData = std::mem::zeroed();
            fcinfo.flinfo = &mut flinfo;
            for _ in 0..20 {
                callsite(&mut fcinfo).matcher = Rc::downgrade(&matcher);
                assert_eq!(Rc::weak_count(&matcher), 1);
                context.reset();
                // A reset discards the executor's FmgrInfo along with its state.
                // Our stack fixture survives, so explicitly discard its pointer.
                flinfo.fn_extra = std::ptr::null_mut();
                assert!(flinfo.fn_extra.is_null());
                assert_eq!(Rc::weak_count(&matcher), 0);
            }
            callsite(&mut fcinfo).matcher = Rc::downgrade(&matcher);
            assert_eq!(Rc::weak_count(&matcher), 1);
        }
        drop(context);
        assert_eq!(Rc::weak_count(&matcher), 0);
    }
}
