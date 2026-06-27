use clippy_utils::diagnostics::span_lint_and_then;
use rustc_hir::def_id::LOCAL_CRATE;
use rustc_hir::{Expr, ExprKind};
use rustc_lint::LateContext;

use crate::TELEMETRY_METRIC_CONSTRUCTION;
use crate::telemetry_literal::{INSTRUMENT_BUILDERS, receiver_is_type};

#[derive(Default)]
pub(crate) struct TelemetryMetricConstruction;

impl TelemetryMetricConstruction {
    pub(crate) fn check_expr<'tcx>(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        // `trogon-semconv`'s generated `build_*` constructors are the one place
        // allowed to open an instrument builder; every other crate must call
        // them rather than restating name, description, and unit inline.
        if cx.tcx.crate_name(LOCAL_CRATE).as_str() == "trogon_semconv" {
            return;
        }

        let ExprKind::MethodCall(segment, receiver, _, _) = expr.kind else {
            return;
        };
        if !INSTRUMENT_BUILDERS.contains(&segment.ident.name.as_str()) {
            return;
        }
        if !receiver_is_type(cx, receiver, "opentelemetry", "Meter") {
            return;
        }

        span_lint_and_then(
            cx,
            TELEMETRY_METRIC_CONSTRUCTION,
            segment.ident.span,
            "metric instrument constructed outside `trogon_semconv`",
            |diag| {
                diag.help("construct the instrument with a generated `trogon_semconv::metric::build_*` constructor instead of opening the builder inline");
            },
        );
    }
}
