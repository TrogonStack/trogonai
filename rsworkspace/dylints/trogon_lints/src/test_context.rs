use std::path::Path;

use clippy_utils::is_in_test;
use rustc_hir::HirId;
use rustc_hir::def::DefKind;
use rustc_hir::def_id::LocalDefId;
use rustc_lint::{LateContext, LintContext};
use rustc_span::{FileName, RealFileName, Span};

/// Whether the node `hir_id`, reported at `span`, sits in test code. Covers
/// `#[test]` functions and `#[cfg(test)]` items, the test and benchmark module
/// families (`test_support`, `mocks`, ... whether inline or file-backed), test
/// file names (`tests.rs`, `parse_tests.rs`), and Cargo `tests/`/`benches/`
/// targets. Mirrors `serde_json_macro`, so a site is test code for the same
/// reasons in every lint that consults this.
pub(crate) fn is_test_context(cx: &LateContext<'_>, hir_id: HirId, span: Span) -> bool {
    is_in_test(cx.tcx, hir_id)
        || is_inside_test_module(cx, cx.tcx.hir_get_parent_item(hir_id).def_id)
        || is_test_or_bench_source(cx, span)
}

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

/// Whether the module name belongs to the test and benchmark module families,
/// whose members reach across the tree by design.
pub(crate) fn is_test_module_name(name: &str) -> bool {
    name == "tests"
        || name.ends_with("_tests")
        || name == "benches"
        || name.ends_with("_benches")
        || name == "test_support"
        || name == "mocks"
        || name == "fixtures"
        || name == "testkit"
        || name.ends_with("_harness")
}

/// Whether the span sits in a test or benchmark source file. The file-name half
/// of `is_test_context`, exposed on its own for callers that have a span but no
/// `HirId`.
pub(crate) fn is_test_or_bench_source(cx: &LateContext<'_>, span: Span) -> bool {
    let file = cx.tcx.sess.source_map().lookup_char_pos(span.lo()).file;
    let FileName::Real(real) = &file.name else {
        return false;
    };
    let Some(path) = real.local_path() else {
        return false;
    };
    // A test target's own modules nest arbitrarily deep, and only the target's
    // root file is guaranteed to sit in the `tests`/`benches` directory, so it
    // answers for every file compiled into that target.
    let crate_root = cx
        .sess()
        .local_crate_source_file()
        .and_then(RealFileName::into_local_path);

    is_test_or_bench_path(path, crate_root.as_deref())
}

fn is_test_or_bench_path(path: &Path, crate_root: Option<&Path>) -> bool {
    let test_stem = matches!(
        path.file_stem().and_then(|stem| stem.to_str()),
        Some(stem) if stem == "tests" || stem.ends_with("_tests")
    );

    test_stem || is_in_test_or_bench_dir(path) || crate_root.is_some_and(is_in_test_or_bench_dir)
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
            "crates/api/tests/api/helpers/pump.rs",
            "crates/api/tests/api/main.rs"
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
