//! Trybuild harness asserting the `tests/ui/*.rs` fixtures fail to compile with the expected
//! diagnostics.

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    #[cfg(coverage_nightly)]
    let compiler_diagnostics = "tests/ui/nightly";
    #[cfg(not(coverage_nightly))]
    let compiler_diagnostics = "tests/ui";

    t.compile_fail(format!("{compiler_diagnostics}/bundle_mismatched_event.rs"));
    t.compile_fail("tests/ui/bundle_mismatched_module.rs");
    t.compile_fail("tests/ui/bundle_mismatched_schema.rs");
    t.compile_fail("tests/ui/bundle_mismatched_version.rs");
    t.compile_fail(format!("{compiler_diagnostics}/not_a_decider.rs"));
}
