use std::fmt;

use reqwest::Url;
use trogon_nats::NatsToken;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_std::{EmptySecretError, NonZeroDuration, SecretString};

#[derive(Clone)]
pub struct TelegramBotToken(SecretString);

#[derive(Debug, thiserror::Error)]
pub enum TelegramBotTokenError {
    #[error("{0}")]
    Empty(#[source] EmptySecretError),
    #[error("must match Telegram bot token format")]
    InvalidFormat,
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
    pub fn new(s: impl AsRef<str>) -> Result<Self, EmptySecretError> {
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

#[derive(Debug, thiserror::Error)]
pub enum TelegramPublicWebhookUrlError {
    #[error("invalid public webhook URL: {0}")]
    Parse(#[source] url::ParseError),
    #[error("invalid public webhook URL: must use https")]
    InsecureScheme,
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
mod tests;
