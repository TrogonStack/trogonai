use super::*;

#[test]
fn accepts_a_reason_and_exposes_it_in_every_rendering() {
    let reason = CredentialDestroyReason::new("credential lifecycle cleanup").unwrap();

    assert_eq!(reason.as_str(), "credential lifecycle cleanup");
    assert_eq!(reason.to_string(), "credential lifecycle cleanup");
    assert_eq!(
        format!("{reason:?}"),
        "CredentialDestroyReason(\"credential lifecycle cleanup\")"
    );
}

#[test]
fn rejects_an_empty_reason() {
    assert_eq!(
        CredentialDestroyReason::new(""),
        Err(CredentialDestroyReasonError::Empty)
    );
}

#[test]
fn rejects_a_reason_over_the_length_limit_counted_in_characters() {
    let at_limit = "é".repeat(MAX_DESTROY_REASON_LEN);
    let over_limit = "é".repeat(MAX_DESTROY_REASON_LEN + 1);

    assert!(CredentialDestroyReason::new(at_limit).is_ok());
    assert_eq!(
        CredentialDestroyReason::new(over_limit),
        Err(CredentialDestroyReasonError::TooLong)
    );
}
