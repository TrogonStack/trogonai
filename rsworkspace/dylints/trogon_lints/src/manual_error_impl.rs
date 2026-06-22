use clippy_utils::diagnostics::span_lint_and_then;
use clippy_utils::sym;
use rustc_hir::{Item, ItemKind};
use rustc_lint::LateContext;

use crate::MANUAL_ERROR_IMPL;

pub(crate) fn check_item<'tcx>(cx: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
    if item.span.from_expansion() {
        return;
    }

    let ItemKind::Impl(impl_) = item.kind else {
        return;
    };

    let Some(of_trait) = impl_.of_trait else {
        return;
    };

    let Some(trait_def_id) = of_trait.trait_ref.trait_def_id() else {
        return;
    };

    if cx.tcx.get_diagnostic_item(sym::Error) != Some(trait_def_id) {
        return;
    }

    span_lint_and_then(
        cx,
        MANUAL_ERROR_IMPL,
        of_trait.trait_ref.path.span,
        "manual `std::error::Error` implementation",
        |diag| {
            diag.help(
                "derive the error with `#[derive(thiserror::Error)]` and `#[error(\"...\")]` instead of \
                 implementing `std::error::Error` by hand",
            );
        },
    );
}
