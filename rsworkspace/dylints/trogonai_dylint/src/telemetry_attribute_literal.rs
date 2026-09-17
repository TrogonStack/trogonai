use clippy_utils::diagnostics::span_lint_and_then;
use rustc_hir::{Expr, ExprKind};
use rustc_lint::LateContext;

use crate::TELEMETRY_ATTRIBUTE_LITERAL;
use crate::telemetry_literal::{receiver_is_type, string_literal_span};

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
        if !receiver_is_type(cx, receiver, "tracing", "Span") {
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
