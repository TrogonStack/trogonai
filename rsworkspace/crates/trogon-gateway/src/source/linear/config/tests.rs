use super::*;

#[test]
fn linear_webhook_secret_roundtrips() {
    let secret = LinearWebhookSecret::new("super-secret").unwrap();
    assert_eq!(secret.as_str(), "super-secret");
}

#[test]
fn linear_webhook_secret_debug_redacts() {
    let secret = LinearWebhookSecret::new("super-secret").unwrap();
    assert_eq!(format!("{secret:?}"), "LinearWebhookSecret(****)");
}
