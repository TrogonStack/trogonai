use std::fmt;

use reqwest::Url;
use trogon_nats::NatsToken;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_std::{EmptySecret, NonZeroDuration, SecretString};

#[derive(Clone)]
pub struct TelegramBotToken(SecretString);

#[derive(Debug)]
pub enum TelegramBotTokenError {
    Empty(EmptySecret),
    InvalidFormat,
}

impl fmt::Display for TelegramBotTokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty(error) => write!(f, "{error}"),
            Self::InvalidFormat => f.write_str("must match Telegram bot token format"),
        }
    }
}

impl std::error::Error for TelegramBotTokenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Empty(error) => Some(error),
            Self::InvalidFormat => None,
        }
    }
}

impl TelegramBotToken {
    pub fn new(s: impl AsRef<str>) -> Result<Self, TelegramBotTokenError> {
        let secret = SecretString::new(s).map_err(TelegramBotTokenError::Empty)?;
        if !is_telegram_bot_token(secret.as_str()) {
            return Err(TelegramBotTokenError::InvalidFormat);
        }
        Ok(Self(secret))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

fn is_telegram_bot_token(value: &str) -> bool {
    let Some((bot_id, token)) = value.split_once(':') else {
        return false;
    };

    !bot_id.is_empty()
        && bot_id.bytes().all(|byte| byte.is_ascii_digit())
        && token.len() >= 20
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

impl fmt::Debug for TelegramBotToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TelegramBotToken(****)")
    }
}

#[derive(Clone)]
pub struct TelegramWebhookSecret(SecretString);

impl TelegramWebhookSecret {
    pub fn new(s: impl AsRef<str>) -> Result<Self, EmptySecret> {
        SecretString::new(s).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for TelegramWebhookSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TelegramWebhookSecret(****)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TelegramPublicWebhookUrl(Url);

#[derive(Debug)]
pub enum TelegramPublicWebhookUrlError {
    Parse(url::ParseError),
    InsecureScheme,
}

impl fmt::Display for TelegramPublicWebhookUrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(error) => write!(f, "invalid public webhook URL: {error}"),
            Self::InsecureScheme => f.write_str("invalid public webhook URL: must use https"),
        }
    }
}

impl std::error::Error for TelegramPublicWebhookUrlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(error) => Some(error),
            Self::InsecureScheme => None,
        }
    }
}

impl TelegramPublicWebhookUrl {
    pub fn new(s: impl AsRef<str>) -> Result<Self, TelegramPublicWebhookUrlError> {
        let url = Url::parse(s.as_ref()).map_err(TelegramPublicWebhookUrlError::Parse)?;
        if url.scheme() != "https" {
            return Err(TelegramPublicWebhookUrlError::InsecureScheme);
        }
        Ok(Self(url))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Clone, Debug)]
pub struct TelegramWebhookRegistrationConfig {
    pub bot_token: TelegramBotToken,
    pub public_webhook_url: TelegramPublicWebhookUrl,
}

pub struct TelegramSourceConfig {
    pub webhook_secret: TelegramWebhookSecret,
    pub registration: Option<TelegramWebhookRegistrationConfig>,
    pub subject_prefix: NatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub nats_ack_timeout: NonZeroDuration,
}

#[cfg(test)]
mod tests {
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
        let url = TelegramPublicWebhookUrl::new("https://example.com/telegram/webhook").unwrap();
        assert_eq!(url.as_str(), "https://example.com/telegram/webhook");
    }

    #[test]
    fn telegram_public_webhook_url_requires_https() {
        let err = TelegramPublicWebhookUrl::new("http://example.com/telegram/webhook").unwrap_err();
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
}
