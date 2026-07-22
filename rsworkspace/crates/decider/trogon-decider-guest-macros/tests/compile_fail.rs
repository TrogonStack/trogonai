//! Trybuild harness asserting the `tests/ui/*.rs` fixtures fail to compile with the expected
//! diagnostics.

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
