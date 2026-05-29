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

#[derive(Debug)]
pub enum SlackAppTokenError {
    Empty(EmptySecret),
    MissingPrefix,
}

impl fmt::Display for SlackAppTokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty(error) => write!(f, "{error}"),
            Self::MissingPrefix => f.write_str("must start with xapp-"),
        }
    }
}

impl std::error::Error for SlackAppTokenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Empty(error) => Some(error),
            Self::MissingPrefix => None,
        }
    }
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
mod tests {
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
}
