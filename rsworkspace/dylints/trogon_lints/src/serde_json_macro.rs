use std::collections::HashSet;
use std::path::{Path, PathBuf};

use clippy_utils::diagnostics::span_lint_and_then;
use clippy_utils::is_in_test;
use clippy_utils::macros::macro_backtrace;
use rustc_hir::Expr;
use rustc_hir::def::DefKind;
use rustc_hir::def_id::LocalDefId;
use rustc_lint::{LateContext, LintContext};
use rustc_span::hygiene::MacroKind;
use rustc_span::{ExpnId, FileName, RealFileName, Span};

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
/// families (`test_support`, `mocks`, ... whether inline or file-backed), test
/// file names (`tests.rs`, `parse_tests.rs`), and Cargo `tests/`/`benches/`
/// targets.
fn is_test_context<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>, span: Span) -> bool {
    is_in_test(cx.tcx, expr.hir_id)
        || is_inside_test_module(cx, cx.tcx.hir_enclosing_body_owner(expr.hir_id))
        || is_test_or_bench_source(cx, span)
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

/// Test module files (`tests.rs`, `*_tests.rs`, the names `test_module_naming`
/// enforces) and files belonging to a Cargo `tests/` or `benches/` target.
/// Mirrors `constant_outside_constants_module`, so a file is test code for the
/// same reasons in both lints.
fn is_test_or_bench_source(cx: &LateContext<'_>, span: Span) -> bool {
    let Some(path) = source_path(cx, span) else {
        return false;
    };
    // A test target's own modules nest arbitrarily deep
    // (`tests/api/helpers/payload.rs`), and only the target's root file is
    // guaranteed to sit in the `tests`/`benches` directory, so it answers for
    // every file compiled into that target.
    let crate_root = cx
        .sess()
        .local_crate_source_file()
        .and_then(RealFileName::into_local_path);

    is_test_or_bench_path(&path, crate_root.as_deref())
}

fn is_test_or_bench_path(path: &Path, crate_root: Option<&Path>) -> bool {
    let test_stem = matches!(
        path.file_stem().and_then(|stem| stem.to_str()),
        Some(stem) if stem == "tests" || stem.ends_with("_tests")
    );

    test_stem || is_in_test_or_bench_dir(path) || crate_root.is_some_and(is_in_test_or_bench_dir)
}

fn source_path(cx: &LateContext<'_>, span: Span) -> Option<PathBuf> {
    let file = cx.tcx.sess.source_map().lookup_char_pos(span.lo()).file;
    match &file.name {
        FileName::Real(real) => real.local_path().map(Path::to_path_buf),
        _ => None,
    }
}

/// Cargo integration-test and benchmark targets sit directly in the crate's
/// `tests`/`benches` directory, or one subdirectory deep (`tests/foo/main.rs`
/// and that target's modules). Only those two positions count, so an unrelated
/// ancestor that happens to be named `tests` (the checkout path, say) does not
/// exempt the whole crate.
fn is_in_test_or_bench_dir(path: &Path) -> bool {
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

// The compiletest harness builds every fixture as a standalone crate in a
// temporary directory, so a Cargo `tests/` target layout cannot be expressed
// there. These cover the path rules directly instead.
#[cfg(test)]
mod tests {
    use super::is_test_or_bench_path;
    use std::path::Path;

    fn is_test_source(path: &str, crate_root: &str) -> bool {
        is_test_or_bench_path(Path::new(path), Some(Path::new(crate_root)))
    }

    #[test]
    fn exempts_test_file_names() {
        assert!(is_test_source(
            "crates/api/src/tests.rs",
            "crates/api/src/lib.rs"
        ));
        assert!(is_test_source(
            "crates/api/src/parse_tests.rs",
            "crates/api/src/lib.rs"
        ));
    }

    #[test]
    fn exempts_nested_modules_of_a_test_target() {
        assert!(is_test_source(
            "crates/api/tests/api/helpers/payload.rs",
            "crates/api/tests/api/main.rs"
        ));
        assert!(is_test_source(
            "crates/api/benches/throughput.rs",
            "crates/api/benches/throughput.rs"
        ));
    }

    #[test]
    fn reports_production_sources() {
        assert!(!is_test_source(
            "crates/api/src/handlers/mod.rs",
            "crates/api/src/lib.rs"
        ));
    }

    // A checkout path that happens to contain a `tests` directory must not
    // exempt the crate compiled out of it.
    #[test]
    fn reports_sources_under_an_unrelated_tests_ancestor() {
        assert!(!is_test_source(
            "/src/tests/checkout/crates/api/src/handlers/mod.rs",
            "/src/tests/checkout/crates/api/src/lib.rs"
        ));
    }
}
