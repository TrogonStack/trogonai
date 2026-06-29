use std::fmt;

use trogon_nats::NatsToken;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_std::{EmptySecret, NonZeroDuration, SecretString};

#[derive(Clone)]
pub struct SlackSigningSecret(SecretString);

impl SlackSigningSecret {
    pub fn new(s: impl AsRef<str>) -> Result<Self, EmptySecret> {
        SecretString::new(s).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for SlackSigningSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SlackSigningSecret(****)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SlackAppTokenError {
    #[error("{0}")]
    Empty(#[source] EmptySecret),
    #[error("must start with xapp-")]
    MissingPrefix,
}

#[derive(Clone)]
pub struct SlackAppToken(SecretString);

impl SlackAppToken {
    pub fn new(s: impl AsRef<str>) -> Result<Self, SlackAppTokenError> {
        let secret = SecretString::new(s).map_err(SlackAppTokenError::Empty)?;
        if !secret.as_str().starts_with("xapp-") {
            return Err(SlackAppTokenError::MissingPrefix);
        }
        Ok(Self(secret))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for SlackAppToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SlackAppToken(****)")
    }
}

#[derive(Clone)]
pub struct SlackWebhookConfig {
    pub signing_secret: SlackSigningSecret,
    pub timestamp_max_drift: NonZeroDuration,
}

#[derive(Clone)]
pub struct SlackSocketModeConfig {
    pub app_token: SlackAppToken,
}

#[derive(Clone)]
pub enum SlackTransportConfig {
    Webhook(SlackWebhookConfig),
    SocketMode(SlackSocketModeConfig),
}

#[derive(Clone)]
pub struct SlackConfig {
    pub subject_prefix: NatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub nats_ack_timeout: NonZeroDuration,
    pub transport: SlackTransportConfig,
}

impl SlackConfig {
    pub fn webhook(&self) -> Option<&SlackWebhookConfig> {
        match &self.transport {
            SlackTransportConfig::Webhook(config) => Some(config),
            SlackTransportConfig::SocketMode(_) => None,
        }
    }

    pub fn socket_mode(&self) -> Option<&SlackSocketModeConfig> {
        match &self.transport {
            SlackTransportConfig::Webhook(_) => None,
            SlackTransportConfig::SocketMode(config) => Some(config),
        }
    }
}

#[cfg(test)]
mod tests;
