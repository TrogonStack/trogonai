use super::*;

#[test]
fn an_unexpected_reply_names_what_was_expected_and_observed() {
    let failure = CheckFailureError::unexpected("a grant", &"nothing");
    assert_eq!(failure.to_string(), "expected a grant, observed \"nothing\"");
}

#[test]
fn an_invalid_identifier_keeps_the_reason() {
    let failure = CheckFailureError::identifier("empty");
    assert_eq!(failure.to_string(), "invalid identifier: empty");
}

#[test]
fn ensure_passes_a_condition_that_holds() {
    assert!(ensure(true, "anything", &1).is_ok());
}

#[test]
fn ensure_fails_a_condition_that_does_not_hold() {
    assert!(matches!(
        ensure(false, "one", &2),
        Err(CheckFailureError::Mismatch { .. })
    ));
}
