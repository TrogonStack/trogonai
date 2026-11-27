use super::*;
use crate::CheckFailureError;

#[test]
fn every_failure_is_listed() {
    let failures = ["one", "two"].map(|name| SuiteFailureError {
        name,
        failure: CheckFailureError::Identifier("bad".to_owned()),
    });
    let shown = NotConformantError(failures.into()).to_string();
    assert_eq!(
        shown,
        "worker is not conformant:\n  one: invalid identifier: bad\n  two: invalid identifier: bad"
    );
}
