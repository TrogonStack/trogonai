use super::*;

#[test]
fn datadog_webhook_token_roundtrips() {
    let token = DatadogWebhookToken::new("super-secret").unwrap();
    assert_eq!(token.as_str(), "super-secret");
}

#[test]
fn datadog_webhook_token_debug_redacts() {
    let token = DatadogWebhookToken::new("super-secret").unwrap();
    assert_eq!(format!("{token:?}"), "DatadogWebhookToken(****)");
}

#[test]
fn datadog_webhook_token_rejects_empty() {
    assert!(DatadogWebhookToken::new("").is_err());
}
