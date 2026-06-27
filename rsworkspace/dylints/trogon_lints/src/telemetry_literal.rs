use rustc_ast::LitKind;
use rustc_hir::{Expr, ExprKind};
use rustc_lint::LateContext;
use rustc_middle::ty;

/// Every `opentelemetry` `Meter` method that opens an instrument builder, keyed
/// by the instrument name as its first argument. Shared by the metric lints.
pub(crate) const INSTRUMENT_BUILDERS: &[&str] = &[
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

/// The span of `expr` when it is a string literal, otherwise `None`. Shared by
/// every telemetry-literal lint so they all caret the quoted string itself.
pub(crate) fn string_literal_span(expr: &Expr<'_>) -> Option<rustc_span::Span> {
    match expr.kind {
        ExprKind::Lit(lit) if matches!(lit.node, LitKind::Str(..)) => Some(lit.span),
        _ => None,
    }
}

/// Whether the adjusted type of `receiver`, with references peeled, is the ADT
/// `krate::...::ty_name`. Telemetry types are identified by their owning crate
/// and name rather than a full def-path, mirroring how the checked crates avoid
/// registering diagnostic items.
pub(crate) fn receiver_is_type<'tcx>(
    cx: &LateContext<'tcx>,
    receiver: &'tcx Expr<'tcx>,
    krate: &str,
    ty_name: &str,
) -> bool {
    let ty = cx.typeck_results().expr_ty_adjusted(receiver).peel_refs();
    let ty::Adt(adt, _) = ty.kind() else {
        return false;
    };
    let did = adt.did();
    cx.tcx.crate_name(did.krate).as_str() == krate && cx.tcx.item_name(did).as_str() == ty_name
}
