use super::*;

#[test]
fn accepts_a_bounded_fingerprint() {
    let fingerprint = CredentialFingerprint::new("memory:abc").unwrap();

    assert_eq!(fingerprint.as_str(), "memory:abc");
}

#[test]
fn rejects_empty_fingerprint() {
    assert_eq!(CredentialFingerprint::new(""), Err(CredentialFingerprintError::Empty));
}

#[test]
fn rejects_fingerprint_over_the_maximum_length() {
    let too_long = "a".repeat(MAX_FINGERPRINT_LEN + 1);

    assert_eq!(
        CredentialFingerprint::new(too_long),
        Err(CredentialFingerprintError::TooLong(MAX_FINGERPRINT_LEN + 1))
    );
}

#[test]
fn accepts_fingerprint_at_the_maximum_length() {
    assert!(CredentialFingerprint::new("a".repeat(MAX_FINGERPRINT_LEN)).is_ok());
}

#[test]
fn debug_names_the_type_and_value() {
    let fingerprint = CredentialFingerprint::new("memory:abc").unwrap();

    assert_eq!(format!("{fingerprint:?}"), "CredentialFingerprint(\"memory:abc\")");
}
