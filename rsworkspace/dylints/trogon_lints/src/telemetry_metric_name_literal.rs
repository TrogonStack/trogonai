use clippy_utils::diagnostics::span_lint_and_then;
use rustc_hir::{Expr, ExprKind};
use rustc_lint::LateContext;

use crate::TELEMETRY_METRIC_NAME_LITERAL;
use crate::telemetry_literal::{receiver_is_type, string_literal_span};

/// Every `Meter` method that takes an instrument name as its first argument.
const INSTRUMENT_BUILDERS: &[&str] = &[
    "u64_counter",
    "f64_counter",
    "u64_observable_counter",
    "f64_observable_counter",
    "i64_up_down_counter",
    "f64_up_down_counter",
    "i64_observable_up_down_counter",
    "f64_observable_up_down_counter",
    "u64_histogram",
    "f64_histogram",
    "i64_histogram",
    "u64_gauge",
    "i64_gauge",
    "f64_gauge",
    "u64_observable_gauge",
    "i64_observable_gauge",
    "f64_observable_gauge",
];

#[derive(Default)]
pub(crate) struct TelemetryMetricNameLiteral;

impl TelemetryMetricNameLiteral {
    pub(crate) fn check_expr<'tcx>(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        let ExprKind::MethodCall(segment, receiver, args, _) = expr.kind else {
            return;
        };
        if !INSTRUMENT_BUILDERS.contains(&segment.ident.name.as_str()) {
            return;
        }
        let Some(name) = args.first() else {
            return;
        };
        let Some(name_span) = string_literal_span(name) else {
            return;
        };
        if !receiver_is_type(cx, receiver, "opentelemetry", "Meter") {
            return;
        }

        span_lint_and_then(
            cx,
            TELEMETRY_METRIC_NAME_LITERAL,
            name_span,
            "metric instrument name written as an inline string literal",
            |diag| {
                diag.help("name the instrument with a generated `trogon_semconv::metric` constant instead of an inline string");
            },
        );
    }
}
