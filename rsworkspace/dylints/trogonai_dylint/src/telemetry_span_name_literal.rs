use clippy_utils::diagnostics::span_lint_and_then;
use rustc_hir::{Expr, ExprKind};
use rustc_lint::LateContext;

use crate::TELEMETRY_SPAN_NAME_LITERAL;
use crate::telemetry_literal::string_literal_span;
use crate::tracing_metadata::metadata_new_kind;

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
        // Both `info_span!`/`span!` and `#[instrument]` lower to a
        // `Metadata::new` call with the span name as the first argument; the
        // event macros lower to the same call with a synthetic literal name but
        // `Kind::EVENT`, so the kind check keeps this from flagging every
        // `info!`/`warn!`.
        let Some(kind) = metadata_new_kind(cx, callee, args) else {
            return;
        };
        if kind.as_str() != "SPAN" {
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
