use super::*;

#[test]
fn exposes_the_verifier_value() {
    let verifier = SecretVerifier::new("hash-of-secret").unwrap();

    assert_eq!(verifier.as_str(), "hash-of-secret");
}

#[test]
fn rejects_empty_verifier() {
    assert!(matches!(SecretVerifier::new(""), Err(SecretVerifierError::Empty(_))));
}

#[test]
fn debug_redacts_the_verifier() {
    let verifier = SecretVerifier::new("hash-of-secret").unwrap();

    let debug = format!("{verifier:?}");

    assert_eq!(debug, "SecretVerifier(***)");
    assert!(!debug.contains("hash-of-secret"));
}
