use super::*;

#[test]
fn telegram_webhook_secret_roundtrips() {
    let secret = TelegramWebhookSecret::new("super-secret").unwrap();
    assert_eq!(secret.as_str(), "super-secret");
}

#[test]
fn telegram_webhook_secret_debug_redacts() {
    let secret = TelegramWebhookSecret::new("super-secret").unwrap();
    assert_eq!(format!("{secret:?}"), "TelegramWebhookSecret(****)");
}

#[test]
fn telegram_bot_token_roundtrips() {
    let token = TelegramBotToken::new("123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ").unwrap();
    assert_eq!(token.as_str(), "123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ");
}

#[test]
fn telegram_bot_token_debug_redacts() {
    let token = TelegramBotToken::new("123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ").unwrap();
    assert_eq!(format!("{token:?}"), "TelegramBotToken(****)");
}

#[test]
fn telegram_bot_token_rejects_empty_secret() {
    let err = TelegramBotToken::new("").unwrap_err();

    assert!(matches!(err, TelegramBotTokenError::Empty(_)));
    assert_eq!(err.to_string(), "secret must not be empty");
    assert!(std::error::Error::source(&err).is_some());
}

#[test]
fn telegram_bot_token_rejects_invalid_shape() {
    let err = TelegramBotToken::new("123:abc").unwrap_err();

    assert!(matches!(err, TelegramBotTokenError::InvalidFormat));
    assert_eq!(err.to_string(), "must match Telegram bot token format");
    assert!(std::error::Error::source(&err).is_none());
}

#[test]
fn telegram_bot_token_rejects_missing_separator() {
    let err = TelegramBotToken::new("not-a-telegram-token").unwrap_err();

    assert!(matches!(err, TelegramBotTokenError::InvalidFormat));
}

#[test]
fn telegram_public_webhook_url_roundtrips() {
    let url = TelegramPublicWebhookUrl::new("https://example.com/sources/telegram/primary/webhook").unwrap();
    assert_eq!(url.as_str(), "https://example.com/sources/telegram/primary/webhook");
}

#[test]
fn telegram_public_webhook_url_requires_https() {
    let err = TelegramPublicWebhookUrl::new("http://example.com/sources/telegram/primary/webhook").unwrap_err();
    assert_eq!(err.to_string(), "invalid public webhook URL: must use https");
    assert!(std::error::Error::source(&err).is_none());
}

#[test]
fn telegram_public_webhook_url_preserves_parse_error_source() {
    let err = TelegramPublicWebhookUrl::new("not a url").unwrap_err();

    assert!(matches!(err, TelegramPublicWebhookUrlError::Parse(_)));
    assert!(err.to_string().starts_with("invalid public webhook URL:"));
    assert!(std::error::Error::source(&err).is_some());
}
