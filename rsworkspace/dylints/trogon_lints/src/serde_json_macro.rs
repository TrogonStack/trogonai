use std::collections::HashSet;

use clippy_utils::diagnostics::span_lint_and_then;
use clippy_utils::is_in_test;
use clippy_utils::macros::macro_backtrace;
use rustc_hir::Expr;
use rustc_hir::def::DefKind;
use rustc_hir::def_id::LocalDefId;
use rustc_lint::LateContext;
use rustc_span::hygiene::MacroKind;
use rustc_span::{ExpnId, FileName, Span};

use crate::SERDE_JSON_MACRO;

#[derive(Default)]
pub(crate) struct SerdeJsonMacro {
    /// Every expression inside a `json!` expansion carries that expansion in its
    /// span, so the pass would otherwise report the same invocation once per
    /// node it produced. Report the first node reached and remember the
    /// expansion.
    reported: HashSet<ExpnId>,
}

impl SerdeJsonMacro {
    pub(crate) fn check_expr<'tcx>(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        // The outermost `serde_json::json!` in the backtrace is the one written
        // by hand: `json!` expands through `json_internal!`, and a `json!`
        // nested in another macro call (`vec![json!({})]`) still has to be
        // reported at its own invocation.
        let Some(call_site) = macro_backtrace(expr.span)
            .filter(|call| {
                call.kind == MacroKind::Bang
                    && cx.tcx.crate_name(call.def_id.krate).as_str() == "serde_json"
                    && cx.tcx.item_name(call.def_id).as_str() == "json"
            })
            .map(|call| (call.expn, call.span))
            .last()
        else {
            return;
        };
        let (expn, span) = call_site;

        if is_test_context(cx, expr, span) || is_generated(cx, span) {
            return;
        }

        if !self.reported.insert(expn) {
            return;
        }

        span_lint_and_then(
            cx,
            SERDE_JSON_MACRO,
            span,
            "JSON value built with the `serde_json::json!` macro",
            |diag| {
                diag.help(
                    "define the payload as a type with `#[derive(serde::Serialize)]` and convert it with \
                     `serde_json::to_value`, so the shape has a name, a schema, and one definition",
                );
                diag.note(
                    "if the shape is genuinely dynamic, opt out at the site with \
                     `#[cfg_attr(dylint_lib = \"trogon_lints\", allow(serde_json_macro, reason = \"...\"))]`",
                );
            },
        );
    }
}

/// Whether the invocation sits in test code, where `json!` is a fixture literal
/// rather than a payload type the production code has to keep in sync. Covers
/// `#[test]` functions and `#[cfg(test)]` items, the test and benchmark module
/// families (whose file-backed modules are `tests.rs`, `parse_tests.rs`,
/// `test_support.rs`, ...), and Cargo `tests/`/`benches/` targets.
fn is_test_context<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>, span: Span) -> bool {
    is_in_test(cx.tcx, expr.hir_id)
        || is_inside_test_module(cx, cx.tcx.hir_enclosing_body_owner(expr.hir_id))
        || is_in_test_or_bench_dir(cx, span)
}

/// Walk the module ancestors of the enclosing body and exempt the invocation
/// when any of them is a test, benchmark, or test-support module. Mirrors the
/// module vocabulary `constant_outside_constants_module` recognizes.
fn is_inside_test_module(cx: &LateContext<'_>, def_id: LocalDefId) -> bool {
    let mut current = def_id.to_def_id();
    while let Some(parent) = cx.tcx.opt_parent(current) {
        if cx.tcx.def_kind(parent) == DefKind::Mod
            && cx
                .tcx
                .opt_item_name(parent)
                .is_some_and(|name| is_test_module_name(name.as_str()))
        {
            return true;
        }
        current = parent;
    }
    false
}

fn is_test_module_name(name: &str) -> bool {
    name == "tests"
        || name.ends_with("_tests")
        || name == "benches"
        || name.ends_with("_benches")
        // The not-for-prod test-support module family: fixtures, fakes, and
        // in-process harnesses that build request and response payloads for
        // tests rather than for production callers.
        || name == "test_support"
        || name == "mocks"
        || name == "fixtures"
        || name == "testkit"
        || name.ends_with("_harness")
}

/// Cargo integration-test and benchmark targets sit directly in the crate's
/// `tests`/`benches` directory, or one subdirectory deep (`tests/foo/main.rs`
/// and that target's modules). Only those two positions count, so an unrelated
/// ancestor that happens to be named `tests` (the checkout path, say) does not
/// exempt the whole crate.
fn is_in_test_or_bench_dir(cx: &LateContext<'_>, span: Span) -> bool {
    let file = cx.tcx.sess.source_map().lookup_char_pos(span.lo()).file;
    let FileName::Real(real) = &file.name else {
        return false;
    };
    let Some(path) = real.local_path() else {
        return false;
    };

    let mut dir = path.parent();
    for _ in 0..2 {
        let Some(current) = dir else {
            break;
        };
        if matches!(
            current.file_name().and_then(|name| name.to_str()),
            Some("tests" | "benches")
        ) {
            return true;
        }
        dir = current.parent();
    }
    false
}

/// Generated files (proto codegen, semconv, ...) carry whatever JSON shape the
/// generator emits and cannot be hand-edited.
fn is_generated(cx: &LateContext<'_>, span: Span) -> bool {
    let file = cx.tcx.sess.source_map().lookup_char_pos(span.lo()).file;
    let Some(src) = &file.src else {
        return false;
    };
    src.lines()
        .take(5)
        .any(|line| line.contains("@generated") || line.contains("DO NOT EDIT"))
}
