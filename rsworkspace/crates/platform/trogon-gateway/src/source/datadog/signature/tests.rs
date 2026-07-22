use super::*;

#[test]
fn error_display_messages() {
    assert_eq!(SignatureError::Missing.to_string(), "missing webhook token header");
    assert_eq!(SignatureError::Mismatch.to_string(), "webhook token mismatch");
}

#[test]
fn valid_token_passes() {
    assert!(verify("my-secret", Some("my-secret")).is_ok());
}

#[test]
fn wrong_token_fails() {
    assert!(matches!(
        verify("correct-secret", Some("wrong-secret")),
        Err(SignatureError::Mismatch)
    ));
}

#[test]
fn missing_token_fails() {
    assert!(matches!(verify("secret", None), Err(SignatureError::Missing)));
}

#[test]
fn empty_secret_does_not_match_nonempty_token() {
    assert!(matches!(verify("", Some("something")), Err(SignatureError::Mismatch)));
}
