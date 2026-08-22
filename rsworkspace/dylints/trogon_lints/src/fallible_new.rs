use std::ops::ControlFlow;

use clippy_utils::diagnostics::span_lint_and_then;
use clippy_utils::macros::{is_panic, root_macro_call_first_node};
use clippy_utils::res::MaybeDef;
use clippy_utils::visitors::for_each_expr;
use clippy_utils::{get_async_fn_body, sym};
use rustc_hir::def::DefKind;
use rustc_hir::def_id::LocalDefId;
use rustc_hir::intravisit::FnKind;
use rustc_hir::{Body, Expr, ExprKind};
use rustc_lint::LateContext;
use rustc_middle::ty::Ty;
use rustc_span::{Ident, Span};

use crate::FALLIBLE_NEW;
use crate::test_context::is_test_context;

/// An operation inside a constructor that ends the process, and how it reads
/// at the site.
struct PanickingOperation {
    span: Span,
    spelling: &'static str,
}

pub(crate) fn check_fn<'tcx>(
    cx: &LateContext<'tcx>,
    kind: FnKind<'tcx>,
    body: &'tcx Body<'tcx>,
    span: Span,
    def_id: LocalDefId,
) {
    if span.from_expansion() {
        return;
    }

    let Some(ident) = constructor_ident(kind) else {
        return;
    };

    // A trait's implementor cannot rename the method or widen its return type,
    // so the choice this lint asks for is not theirs to make.
    if is_trait_impl_item(cx, def_id) || admits_failure(cx, body, def_id) {
        return;
    }

    if is_test_context(cx, cx.tcx.local_def_id_to_hir_id(def_id), span) {
        return;
    }

    let Some(operation) = first_panicking_operation(cx, body) else {
        return;
    };

    let name = ident.name;
    span_lint_and_then(
        cx,
        FALLIBLE_NEW,
        ident.span,
        format!(
            "constructor `{name}` can panic, and its signature gives the caller no failure to handle"
        ),
        |diag| {
            diag.span_note(
                operation.span,
                format!(
                    "`{}` here ends the process instead of returning to the caller",
                    operation.spelling
                ),
            );
            diag.help(
                "return `Result<Self, _>` and propagate the failure with `?`, renaming the constructor to \
                 `try_new` (or `try_new_*`) when an infallible one stays beside it, or move the fallible work \
                 out to the caller",
            );
            diag.note(
                "if the panic is an invariant the caller cannot break, opt out at the site with \
                 `#[cfg_attr(dylint_lib = \"trogon_lints\", allow(fallible_new, reason = \"...\"))]`",
            );
        },
    );
}

/// The name of the function under inspection, when it is one the `new`
/// convention speaks for. `new_*` variants (`new_with_capacity`,
/// `new_unchecked`) carry the same promise as `new` and are covered too.
fn constructor_ident(kind: FnKind<'_>) -> Option<Ident> {
    let ident = match kind {
        FnKind::ItemFn(ident, ..) | FnKind::Method(ident, _) => ident,
        FnKind::Closure => return None,
    };

    let name = ident.name.as_str();
    (name == "new" || name.starts_with("new_")).then_some(ident)
}

fn is_trait_impl_item(cx: &LateContext<'_>, def_id: LocalDefId) -> bool {
    cx.tcx
        .opt_parent(def_id.to_def_id())
        .is_some_and(|parent| matches!(cx.tcx.def_kind(parent), DefKind::Impl { of_trait: true }))
}

/// Whether the signature already tells the caller construction can fail, which
/// is one of the two fixes the lint asks for and so needs no report.
fn admits_failure<'tcx>(
    cx: &LateContext<'tcx>,
    body: &'tcx Body<'tcx>,
    def_id: LocalDefId,
) -> bool {
    let output = output_ty(cx, body, def_id);
    output.is_diag_item(cx, sym::Result) || output.is_diag_item(cx, sym::Option)
}

/// The type the caller receives. An `async fn` reports an opaque future in its
/// signature, so read the type its body evaluates to instead: an
/// `async fn new() -> Result<Self, E>` admits failure just as the blocking one
/// does.
fn output_ty<'tcx>(cx: &LateContext<'tcx>, body: &'tcx Body<'tcx>, def_id: LocalDefId) -> Ty<'tcx> {
    if cx.tcx.asyncness(def_id).is_async()
        && let Some(awaited) = get_async_fn_body(cx.tcx, body)
    {
        return cx.typeck_results().expr_ty(awaited);
    }

    cx.tcx
        .fn_sig(def_id)
        .instantiate_identity()
        .output()
        .skip_binder()
}

/// The first operation in the body that aborts rather than returns. One report
/// per constructor is enough, since the fix is to the signature and covers
/// every such operation at once.
///
/// The walk enters closures and `async` blocks written in the body, because
/// they are the constructor's own code however the panic is scheduled, and
/// stops at nested items, which are separate functions with their own
/// contract.
fn first_panicking_operation<'tcx>(
    cx: &LateContext<'tcx>,
    body: &'tcx Body<'tcx>,
) -> Option<PanickingOperation> {
    for_each_expr(cx, body.value, |expr| match panicking_operation(cx, expr) {
        Some(operation) => ControlFlow::Break(operation),
        None => ControlFlow::Continue(()),
    })
}

fn panicking_operation<'tcx>(
    cx: &LateContext<'tcx>,
    expr: &'tcx Expr<'tcx>,
) -> Option<PanickingOperation> {
    if let Some(macro_call) = root_macro_call_first_node(cx, expr) {
        // `todo!` and `unimplemented!` are left to rustc's own lints, which
        // already report unfinished code wherever it sits.
        let spelling = if is_panic(cx, macro_call.def_id) {
            "panic!"
        } else if cx
            .tcx
            .is_diagnostic_item(sym::unreachable_macro, macro_call.def_id)
        {
            "unreachable!"
        } else {
            return None;
        };

        return Some(PanickingOperation {
            span: macro_call.span,
            spelling,
        });
    }

    let ExprKind::MethodCall(segment, receiver, ..) = expr.kind else {
        return None;
    };
    let spelling = match segment.ident.name.as_str() {
        "unwrap" => "unwrap()",
        "expect" => "expect()",
        _ => return None,
    };

    // `unwrap` and `expect` are ordinary names any type may take; only the two
    // that discard an absent value make the call a panic.
    let receiver_ty = cx.typeck_results().expr_ty(receiver).peel_refs();
    if !receiver_ty.is_diag_item(cx, sym::Result) && !receiver_ty.is_diag_item(cx, sym::Option) {
        return None;
    }

    Some(PanickingOperation {
        span: segment.ident.span,
        spelling,
    })
}
