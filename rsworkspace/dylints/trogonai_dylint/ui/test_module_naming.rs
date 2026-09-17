// edition:2024
// compile-flags: --test

// The sole test module of a parent, named `tests`: must NOT be linted.
#[path = "auxiliary/tmn_tests.rs"]
mod tests;

// A sibling test module named `*_tests`: must NOT be linted.
#[path = "auxiliary/tmn_parse_tests.rs"]
mod parse_tests;

// A module that holds tests but is misnamed: this is the violation.
#[path = "auxiliary/tmn_helpers.rs"]
mod helpers;

// A test-support module (no `#[test]` functions): must NOT be linted even
// though it is not named `tests`/`*_tests`.
#[path = "auxiliary/tmn_mocks.rs"]
mod mocks;
