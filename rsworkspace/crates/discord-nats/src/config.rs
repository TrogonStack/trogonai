//! Configuration types for Discord NATS integration

use serde::{Deserialize, Serialize};

/// NATS connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NatsConfig {
    /// NATS server URLs (comma-separated when loaded from env)
    pub servers: Vec<String>,
    /// NATS prefix for subjects (e.g., "prod", "dev")
    #[serde(default = "default_prefix")]
    pub prefix: String,
    /// Optional credentials file path
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credentials_file: Option<String>,
    /// Optional username
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Optional password
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
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

    /// Parse servers from a comma-separated URL string
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_has_localhost() {
        let cfg = NatsConfig::default();
        assert_eq!(cfg.servers, vec!["localhost:4222"]);
        assert_eq!(cfg.prefix, "prod");
        assert!(cfg.credentials_file.is_none());
    }

    #[test]
    fn test_from_url_single() {
        let cfg = NatsConfig::from_url("nats://localhost:4222", "dev");
        assert_eq!(cfg.servers, vec!["nats://localhost:4222"]);
        assert_eq!(cfg.prefix, "dev");
    }

    #[test]
    fn test_from_url_multiple() {
        let cfg = NatsConfig::from_url("n1:4222,n2:4222,n3:4222", "prod");
        assert_eq!(cfg.servers, vec!["n1:4222", "n2:4222", "n3:4222"]);
    }

    #[test]
    fn test_from_url_trims_whitespace() {
        let cfg = NatsConfig::from_url("n1:4222 , n2:4222", "test");
        assert_eq!(cfg.servers, vec!["n1:4222", "n2:4222"]);
    }

    #[test]
    fn test_with_credentials() {
        let cfg = NatsConfig::from_url("localhost:4222", "test").with_credentials("/path/to/creds");
        assert_eq!(cfg.credentials_file, Some("/path/to/creds".to_string()));
    }

    #[test]
    fn test_with_auth() {
        let cfg = NatsConfig::from_url("localhost:4222", "test").with_auth("alice", "secret");
        assert_eq!(cfg.username, Some("alice".to_string()));
        assert_eq!(cfg.password, Some("secret".to_string()));
    }

    #[test]
    fn test_serde_roundtrip() {
        let cfg = NatsConfig::from_url("localhost:4222", "test");
        let json = serde_json::to_string(&cfg).unwrap();
        let back: NatsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.servers, cfg.servers);
        assert_eq!(back.prefix, cfg.prefix);
    }

    #[test]
    fn test_optional_fields_omitted_in_json() {
        let cfg = NatsConfig::from_url("localhost:4222", "test");
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(!json.contains("credentials_file"));
        assert!(!json.contains("username"));
        assert!(!json.contains("password"));
    }

    #[test]
    fn test_default_prefix_on_deserialization() {
        let json = r#"{"servers":["localhost:4222"]}"#;
        let cfg: NatsConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.prefix, "prod");
    }
}
