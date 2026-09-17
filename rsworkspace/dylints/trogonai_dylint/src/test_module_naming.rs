use clippy_utils::diagnostics::span_lint_and_then;
use clippy_utils::is_test_function;
use rustc_hir::{Item, ItemKind, Mod};
use rustc_lint::LateContext;

use crate::TEST_MODULE_NAMING;

pub(crate) fn check_item<'tcx>(cx: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
    if item.span.from_expansion() {
        return;
    }

    let ItemKind::Mod(ident, module) = item.kind else {
        return;
    };

    // Only police modules that actually hold tests. Test-support modules
    // (mocks, fixtures, testkit) are `#[cfg(test)]`-gated too, but legitimately
    // carry other names because they contain no `#[test]` functions.
    if !module_holds_tests(cx, module) {
        return;
    }

    let name = ident.as_str();
    if name == "tests" || name.ends_with("_tests") {
        return;
    }

    span_lint_and_then(
        cx,
        TEST_MODULE_NAMING,
        item.span.with_hi(ident.span.hi()),
        format!("test module `{ident}` is not named `tests` or `*_tests`"),
        |diag| {
            diag.help(format!(
                "rename `{ident}` to `tests` (the sole test module for its parent) or to `{ident}_tests` \
                 (a sibling test module), and rename its file to match"
            ));
        },
    );
}

/// A module "holds tests" when any direct child item is a test function.
/// `#[test]` and `#[tokio::test]` both lower to a harness-marked function, so
/// `is_test_function` recognizes either.
fn module_holds_tests<'tcx>(cx: &LateContext<'tcx>, module: &'tcx Mod<'tcx>) -> bool {
    module
        .item_ids
        .iter()
        .any(|item_id| is_test_function(cx.tcx, item_id.owner_id.def_id))
}
