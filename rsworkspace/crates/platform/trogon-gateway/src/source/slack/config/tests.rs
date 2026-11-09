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

#[test]
fn api_base_url_joins_the_connections_open_method() {
    let url = SlackApiBaseUrl::new("https://slack.com/api/").unwrap();
    assert_eq!(
        url.connections_open_url().as_str(),
        "https://slack.com/api/apps.connections.open"
    );
}

#[test]
fn api_base_url_without_a_trailing_slash_keeps_its_path() {
    let url = SlackApiBaseUrl::new("https://slack-gov.com/api").unwrap();
    assert_eq!(
        url.connections_open_url().as_str(),
        "https://slack-gov.com/api/apps.connections.open"
    );
}

#[test]
fn api_base_url_accepts_plain_http_for_local_gateways() {
    let url = SlackApiBaseUrl::new("http://127.0.0.1:8080").unwrap();
    assert_eq!(
        url.connections_open_url().as_str(),
        "http://127.0.0.1:8080/apps.connections.open"
    );
}

#[test]
fn api_base_url_must_be_an_absolute_url() {
    let error = SlackApiBaseUrl::new("slack.com/api").unwrap_err();
    assert!(matches!(error, SlackApiBaseUrlError::Parse(_)));
    assert!(error.source().is_some());
}

#[test]
fn api_base_url_must_use_http_or_https() {
    let error = SlackApiBaseUrl::new("ftp://slack.com/api/").unwrap_err();
    assert!(matches!(error, SlackApiBaseUrlError::UnsupportedScheme));
    assert!(error.source().is_none());
}
