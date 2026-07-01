use clippy_utils::diagnostics::span_lint_and_then;
use rustc_hir::def::{DefKind, Res};
use rustc_hir::{Expr, ExprKind};
use rustc_lint::LateContext;
use rustc_middle::ty;

use crate::TELEMETRY_KEY_VALUE_LITERAL;
use crate::telemetry_literal::{in_test_file, string_literal_span};

#[derive(Default)]
pub(crate) struct TelemetryKeyValueLiteral;

impl TelemetryKeyValueLiteral {
    pub(crate) fn check_expr<'tcx>(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        let ExprKind::Call(callee, args) = expr.kind else {
            return;
        };
        let Some(key) = args.first() else {
            return;
        };
        let Some(key_span) = string_literal_span(key) else {
            return;
        };
        if !callee_is_key_value_new(cx, callee) {
            return;
        }
        if in_test_file(cx, key_span) {
            return;
        }

        span_lint_and_then(
            cx,
            TELEMETRY_KEY_VALUE_LITERAL,
            key_span,
            "telemetry attribute key written as an inline string literal",
            |diag| {
                diag.help("pass a generated `trogon_semconv::attribute` constant as the `KeyValue` key instead of an inline string");
            },
        );
    }
}

/// Whether `callee` resolves to `opentelemetry::KeyValue::new`. The associated
/// function's parent impl carries the `KeyValue` self type, which is matched by
/// owning crate and name.
fn callee_is_key_value_new<'tcx>(cx: &LateContext<'tcx>, callee: &'tcx Expr<'tcx>) -> bool {
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
    let did = adt.did();
    cx.tcx.crate_name(did.krate).as_str() == "opentelemetry"
        && cx.tcx.item_name(did).as_str() == "KeyValue"
}
