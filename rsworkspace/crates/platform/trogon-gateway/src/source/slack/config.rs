use std::fmt;

use reqwest::Url;

use super::ack_budget::AckBudget;
use super::constants::APPS_CONNECTIONS_OPEN_METHOD;

use trogon_nats::NatsToken;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_std::{EmptySecretError, HttpBodySizeMax, NonZeroDuration, SecretString};

#[derive(Clone)]
pub struct SlackSigningSecret(SecretString);

impl SlackSigningSecret {
    pub fn new(s: impl AsRef<str>) -> Result<Self, EmptySecretError> {
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
    Empty(#[source] EmptySecretError),
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

#[derive(Debug, thiserror::Error)]
pub enum SlackApiBaseUrlError {
    #[error("invalid Slack API base URL: {0}")]
    Parse(#[source] url::ParseError),
    #[error("invalid Slack API base URL: must use http or https")]
    UnsupportedScheme,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlackApiBaseUrl {
    connections_open: Url,
}

impl SlackApiBaseUrl {
    pub fn new(s: impl AsRef<str>) -> Result<Self, SlackApiBaseUrlError> {
        let mut base = Url::parse(s.as_ref()).map_err(SlackApiBaseUrlError::Parse)?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(SlackApiBaseUrlError::UnsupportedScheme);
        }
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let connections_open = base
            .join(APPS_CONNECTIONS_OPEN_METHOD)
            .map_err(SlackApiBaseUrlError::Parse)?;
        Ok(Self { connections_open })
    }

    pub fn connections_open_url(&self) -> &Url {
        &self.connections_open
    }
}

#[derive(Clone)]
pub struct SlackWebhookConfig {
    pub signing_secret: SlackSigningSecret,
    pub timestamp_max_drift: NonZeroDuration,
    pub max_body_size: HttpBodySizeMax,
}

#[derive(Clone)]
pub struct SlackSocketModeConfig {
    pub app_token: SlackAppToken,
    pub api_base_url: SlackApiBaseUrl,
}

#[derive(Clone)]
pub enum SlackTransportConfig {
    Webhook(SlackWebhookConfig),
    SocketMode(SlackSocketModeConfig),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlackTransport {
    Webhook,
    SocketMode,
}

impl SlackTransport {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Webhook => "webhook",
            Self::SocketMode => "socket_mode",
        }
    }
}

#[derive(Clone)]
pub struct SlackConfig {
    pub subject_prefix: NatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub ack_budget: AckBudget,
    pub transport: SlackTransportConfig,
}

impl SlackConfig {
    pub fn transport_kind(&self) -> SlackTransport {
        match &self.transport {
            SlackTransportConfig::Webhook(_) => SlackTransport::Webhook,
            SlackTransportConfig::SocketMode(_) => SlackTransport::SocketMode,
        }
    }

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
