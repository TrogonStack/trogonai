//! Configuration types for Telegram NATS integration

use serde::{Deserialize, Serialize};

/// NATS connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NatsConfig {
    /// NATS server URLs (comma-separated)
    pub servers: Vec<String>,
    /// NATS prefix for subjects (e.g., "prod", "dev")
    #[serde(default = "default_prefix")]
    pub prefix: String,
    /// Optional credentials
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credentials_file: Option<String>,
    /// Optional username
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Optional password
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// Telegram-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// NATS configuration
    pub nats: NatsConfig,
}

fn default_prefix() -> String {
    "prod".to_string()
}

impl Default for NatsConfig {
    fn default() -> Self {
        Self {
            servers: vec!["localhost:4222".to_string()],
            prefix: default_prefix(),
            credentials_file: None,
            username: None,
            password: None,
        }
    }
}

impl NatsConfig {
    /// Create a new NATS config with the given servers and prefix
    pub fn new(servers: Vec<String>, prefix: impl Into<String>) -> Self {
        Self {
            servers,
            prefix: prefix.into(),
            credentials_file: None,
            username: None,
            password: None,
        }
    }

    /// Parse servers from a comma-separated string
    pub fn from_url(url: impl AsRef<str>, prefix: impl Into<String>) -> Self {
        let servers = url
            .as_ref()
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
        Self::new(servers, prefix)
    }

    /// Set credentials file
    pub fn with_credentials(mut self, file: impl Into<String>) -> Self {
        self.credentials_file = Some(file.into());
        self
    }

    /// Set username and password
    pub fn with_auth(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self.password = Some(password.into());
        self
    }
}
