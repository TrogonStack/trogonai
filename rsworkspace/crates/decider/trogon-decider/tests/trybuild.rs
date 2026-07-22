//! Compile-fail and compile-pass cases for `TestCase`'s given/when/then typestate chain.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[test]
fn given_when_then_typestate() {
    let tests = trybuild::TestCases::new();
    tests.pass("tests/ui/pass/*.rs");
    tests.compile_fail("tests/ui/fail/*.rs");
}
