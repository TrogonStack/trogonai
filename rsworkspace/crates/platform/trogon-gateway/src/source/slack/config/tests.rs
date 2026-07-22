use super::*;
use std::error::Error;

#[test]
fn slack_signing_secret_roundtrips() {
    let secret = SlackSigningSecret::new("super-secret").unwrap();
    assert_eq!(secret.as_str(), "super-secret");
}

#[test]
fn slack_signing_secret_debug_redacts() {
    let secret = SlackSigningSecret::new("super-secret").unwrap();
    assert_eq!(format!("{secret:?}"), "SlackSigningSecret(****)");
}

#[test]
fn slack_app_token_roundtrips() {
    let token = SlackAppToken::new("xapp-test-token").unwrap();
    assert_eq!(token.as_str(), "xapp-test-token");
}

#[test]
fn slack_app_token_debug_redacts() {
    let token = SlackAppToken::new("xapp-test-token").unwrap();
    assert_eq!(format!("{token:?}"), "SlackAppToken(****)");
}

#[test]
fn slack_app_token_requires_app_prefix() {
    let error = SlackAppToken::new("xoxb-not-app-token").unwrap_err();
    assert_eq!(error.to_string(), "must start with xapp-");
    assert!(error.source().is_none());
}

#[test]
fn slack_app_token_rejects_empty_token() {
    let error = SlackAppToken::new("").unwrap_err();
    assert_eq!(error.to_string(), "secret must not be empty");
    assert!(error.source().is_some());
}
