use super::*;

#[test]
fn accepts_a_reason_and_exposes_it_in_every_rendering() {
    let reason = CredentialFailureReason::new("openbao rotate failed").unwrap();

    assert_eq!(reason.as_str(), "openbao rotate failed");
    assert_eq!(reason.to_string(), "openbao rotate failed");
    assert_eq!(
        format!("{reason:?}"),
        "CredentialFailureReason(\"openbao rotate failed\")"
    );
}

#[test]
fn rejects_an_empty_reason() {
    assert_eq!(
        CredentialFailureReason::new(""),
        Err(CredentialFailureReasonError::Empty)
    );
}

#[test]
fn rejects_a_reason_over_the_length_limit_counted_in_characters() {
    assert!(CredentialFailureReason::new("é".repeat(512)).is_ok());
    assert_eq!(
        CredentialFailureReason::new("é".repeat(513)),
        Err(CredentialFailureReasonError::TooLong)
    );
}

#[test]
fn names_the_secret_store_write_failure() {
    let reason = CredentialFailureReason::secret_store_write_failed();

    assert_eq!(reason.as_str(), "secret store write failed");
    assert_eq!(CredentialFailureReason::new("secret store write failed"), Ok(reason));
}
