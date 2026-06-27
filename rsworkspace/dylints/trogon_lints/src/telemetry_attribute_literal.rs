use clippy_utils::diagnostics::span_lint_and_then;
use rustc_ast::LitKind;
use rustc_hir::{Expr, ExprKind};
use rustc_lint::LateContext;
use rustc_middle::ty;

use crate::TELEMETRY_ATTRIBUTE_LITERAL;

#[derive(Default)]
pub(crate) struct TelemetryAttributeLiteral;

impl TelemetryAttributeLiteral {
    pub(crate) fn check_expr<'tcx>(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        let ExprKind::MethodCall(segment, receiver, args, _) = expr.kind else {
            return;
        };
        if segment.ident.name.as_str() != "record" {
            return;
        }
        let Some(key) = args.first() else {
            return;
        };
        let Some(key_span) = string_literal_span(key) else {
            return;
        };
        if !receiver_is_tracing_span(cx, receiver) {
            return;
        }

        span_lint_and_then(
            cx,
            TELEMETRY_ATTRIBUTE_LITERAL,
            key_span,
            "telemetry attribute key written as an inline string literal",
            |diag| {
                diag.help("record the field with a generated `trogon_semconv::attribute` constant instead of an inline string");
            },
        );
    }
}

fn string_literal_span(expr: &Expr<'_>) -> Option<rustc_span::Span> {
    match expr.kind {
        ExprKind::Lit(lit) if matches!(lit.node, LitKind::Str(..)) => Some(lit.span),
        _ => None,
    }
}

fn receiver_is_tracing_span<'tcx>(cx: &LateContext<'tcx>, receiver: &'tcx Expr<'tcx>) -> bool {
    let ty = cx.typeck_results().expr_ty_adjusted(receiver).peel_refs();
    let ty::Adt(adt, _) = ty.kind() else {
        return false;
    };
    let did = adt.did();
    cx.tcx.crate_name(did.krate).as_str() == "tracing" && cx.tcx.item_name(did).as_str() == "Span"
}
