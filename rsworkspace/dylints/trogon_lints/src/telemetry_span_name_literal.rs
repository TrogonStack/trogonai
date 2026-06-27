use clippy_utils::diagnostics::span_lint_and_then;
use rustc_hir::def::{DefKind, Res};
use rustc_hir::{Expr, ExprKind, QPath};
use rustc_lint::LateContext;
use rustc_middle::ty;
use rustc_span::Symbol;

use crate::TELEMETRY_SPAN_NAME_LITERAL;
use crate::telemetry_literal::string_literal_span;

#[derive(Default)]
pub(crate) struct TelemetrySpanNameLiteral;

impl TelemetrySpanNameLiteral {
    pub(crate) fn check_expr<'tcx>(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        let ExprKind::Call(callee, args) = expr.kind else {
            return;
        };
        let Some(name) = args.first() else {
            return;
        };
        let Some(name_span) = string_literal_span(name) else {
            return;
        };
        if !is_span_metadata_new(cx, callee, args) {
            return;
        }

        span_lint_and_then(
            cx,
            TELEMETRY_SPAN_NAME_LITERAL,
            name_span,
            "span name written as an inline string literal",
            |diag| {
                diag.help("name the span with a generated `trogon_semconv::span` constant instead of an inline string");
            },
        );
    }
}

/// Whether `callee` is `tracing_core::Metadata::new` and the call's `kind` (last)
/// argument is `Kind::SPAN`. Both `info_span!`/`span!` and `#[instrument]` lower
/// to such a call with the span name as the first argument; event macros lower
/// to the same call with a synthetic literal name but `Kind::EVENT`, so the kind
/// check keeps this from flagging every `info!`/`warn!`.
fn is_span_metadata_new<'tcx>(
    cx: &LateContext<'tcx>,
    callee: &'tcx Expr<'tcx>,
    args: &'tcx [Expr<'tcx>],
) -> bool {
    let ExprKind::Path(qpath) = &callee.kind else {
        return false;
    };
    let Res::Def(DefKind::AssocFn, did) = cx.qpath_res(qpath, callee.hir_id) else {
        return false;
    };
    if cx.tcx.item_name(did).as_str() != "new" {
        return false;
    }
    let self_ty = cx
        .tcx
        .type_of(cx.tcx.parent(did))
        .instantiate_identity()
        .peel_refs();
    let ty::Adt(adt, _) = self_ty.kind() else {
        return false;
    };
    let adt_did = adt.did();
    if cx.tcx.crate_name(adt_did.krate).as_str() != "tracing_core"
        || cx.tcx.item_name(adt_did).as_str() != "Metadata"
    {
        return false;
    }
    args.last().and_then(path_last_segment) == Some(Symbol::intern("SPAN"))
}

fn path_last_segment(expr: &Expr<'_>) -> Option<Symbol> {
    match &expr.kind {
        ExprKind::Path(QPath::Resolved(_, path)) => path.segments.last().map(|seg| seg.ident.name),
        ExprKind::Path(QPath::TypeRelative(_, seg)) => Some(seg.ident.name),
        _ => None,
    }
}
