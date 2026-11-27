use std::error::Error;

use super::*;

fn failure() -> SuiteFailureError {
    SuiteFailureError {
        name: "a_check",
        failure: CheckFailureError::Identifier("bad".to_owned()),
    }
}

#[test]
fn a_failure_is_shown_by_check_name() {
    assert_eq!(failure().to_string(), "a_check: invalid identifier: bad");
}

#[test]
fn a_failure_is_caused_by_its_check_failure() {
    assert!(failure().source().is_some());
}
