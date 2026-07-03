use super::*;
use std::error::Error;

fn valid_signing_token() -> &'static str {
    "whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE="
}

#[test]
fn gitlab_signing_token_decodes_key() {
    let token = GitLabSigningToken::new(valid_signing_token()).unwrap();
    assert_eq!(token.as_bytes(), b"01234567890123456789012345678901");
    assert_eq!(token.as_ref(), b"01234567890123456789012345678901");
}

#[test]
fn gitlab_signing_token_rejects_bare_key() {
    let err = GitLabSigningToken::new("MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE=").unwrap_err();
    assert_eq!(err.to_string(), "signing token must start with whsec_");
    assert!(err.source().is_none());
}

#[test]
fn gitlab_signing_token_rejects_empty_prefixed_key() {
    let err = GitLabSigningToken::new("whsec_").unwrap_err();
    assert_eq!(err.to_string(), "signing token must not be empty");
    assert!(err.source().is_none());
}

#[test]
fn gitlab_signing_token_rejects_invalid_base64() {
    let err = GitLabSigningToken::new("whsec_not-base64!").unwrap_err();
    assert_eq!(err.to_string(), "signing token must be valid base64");
    assert!(err.source().is_some());
}

#[test]
fn gitlab_signing_token_rejects_wrong_key_length() {
    let err = GitLabSigningToken::new("whsec_dGVzdA==").unwrap_err();
    assert_eq!(err.to_string(), "signing token must decode to 32 bytes, got 4");
    assert!(err.source().is_none());
}

#[test]
fn gitlab_signing_token_debug_redacts() {
    let token = GitLabSigningToken::new(valid_signing_token()).unwrap();
    assert_eq!(format!("{token:?}"), "GitLabSigningToken(****)");
}
