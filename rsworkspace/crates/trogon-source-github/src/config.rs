use std::time::Duration;

use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

use crate::constants::{
    DEFAULT_NATS_ACK_TIMEOUT, DEFAULT_PORT, DEFAULT_STREAM_MAX_AGE, DEFAULT_STREAM_NAME,
    DEFAULT_SUBJECT_PREFIX,
};

/// Configuration for the GitHub webhook server.
///
/// Resolved from environment variables:
/// - `GITHUB_WEBHOOK_SECRET`: HMAC-SHA256 secret configured in GitHub (**required**)
/// - `GITHUB_WEBHOOK_PORT`: HTTP listening port (default: 8080)
/// - `GITHUB_SUBJECT_PREFIX`: NATS subject prefix (default: `github`)
/// - `GITHUB_STREAM_NAME`: JetStream stream name (default: `GITHUB`)
/// - `GITHUB_STREAM_MAX_AGE_SECS`: max age of messages in the JetStream stream in seconds (default: 604800 / 7 days)
/// - `GITHUB_NATS_ACK_TIMEOUT_SECS`: NATS ack timeout in seconds (default: 10)
/// - Standard `NATS_*` variables for NATS connection (see `trogon-nats`)
pub struct GithubConfig {
    pub webhook_secret: String,
    pub port: u16,
    pub subject_prefix: String,
    pub stream_name: String,
    pub stream_max_age: Duration,
    pub nats_ack_timeout: Duration,
    pub nats: NatsConfig,
}

impl GithubConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        Self {
            webhook_secret: env
                .var("GITHUB_WEBHOOK_SECRET")
                .ok()
                .filter(|s| !s.is_empty())
                .expect("GITHUB_WEBHOOK_SECRET is required"),
            port: env
                .var("GITHUB_WEBHOOK_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            subject_prefix: env
                .var("GITHUB_SUBJECT_PREFIX")
                .unwrap_or_else(|_| DEFAULT_SUBJECT_PREFIX.to_string()),
            stream_name: env
                .var("GITHUB_STREAM_NAME")
                .unwrap_or_else(|_| DEFAULT_STREAM_NAME.to_string()),
            stream_max_age: env
                .var("GITHUB_STREAM_MAX_AGE_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_STREAM_MAX_AGE),
            nats_ack_timeout: env
                .var("GITHUB_NATS_ACK_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_NATS_ACK_TIMEOUT),
            nats: NatsConfig::from_env(env),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;

    fn env_with_secret() -> InMemoryEnv {
        let env = InMemoryEnv::new();
        env.set("GITHUB_WEBHOOK_SECRET", "test-secret");
        env
    }

    #[test]
    fn defaults_with_required_secret() {
        let env = env_with_secret();
        let config = GithubConfig::from_env(&env);

        assert_eq!(config.webhook_secret, "test-secret");
        assert_eq!(config.port, 8080);
        assert_eq!(config.subject_prefix, "github");
        assert_eq!(config.stream_name, "GITHUB");
        assert_eq!(config.stream_max_age, Duration::from_secs(7 * 24 * 60 * 60));
        assert_eq!(config.nats_ack_timeout, Duration::from_secs(10));
    }

    #[test]
    fn reads_all_env_vars() {
        let env = InMemoryEnv::new();
        env.set("GITHUB_WEBHOOK_SECRET", "my-secret");
        env.set("GITHUB_WEBHOOK_PORT", "9090");
        env.set("GITHUB_SUBJECT_PREFIX", "gh");
        env.set("GITHUB_STREAM_NAME", "GH_EVENTS");
        env.set("GITHUB_STREAM_MAX_AGE_SECS", "3600");
        env.set("GITHUB_NATS_ACK_TIMEOUT_SECS", "30");

        let config = GithubConfig::from_env(&env);

        assert_eq!(config.webhook_secret, "my-secret");
        assert_eq!(config.port, 9090);
        assert_eq!(config.subject_prefix, "gh");
        assert_eq!(config.stream_name, "GH_EVENTS");
        assert_eq!(config.stream_max_age, Duration::from_secs(3600));
        assert_eq!(config.nats_ack_timeout, Duration::from_secs(30));
    }

    #[test]
    #[should_panic(expected = "GITHUB_WEBHOOK_SECRET is required")]
    fn missing_webhook_secret_panics() {
        let env = InMemoryEnv::new();
        GithubConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "GITHUB_WEBHOOK_SECRET is required")]
    fn empty_webhook_secret_panics() {
        let env = InMemoryEnv::new();
        env.set("GITHUB_WEBHOOK_SECRET", "");
        GithubConfig::from_env(&env);
    }

    #[test]
    fn invalid_port_falls_back_to_default() {
        let env = env_with_secret();
        env.set("GITHUB_WEBHOOK_PORT", "not-a-number");

        let config = GithubConfig::from_env(&env);

        assert_eq!(config.port, 8080);
    }

    #[test]
    fn invalid_max_age_falls_back_to_default() {
        let env = env_with_secret();
        env.set("GITHUB_STREAM_MAX_AGE_SECS", "not-a-number");

        let config = GithubConfig::from_env(&env);

        assert_eq!(config.stream_max_age, DEFAULT_STREAM_MAX_AGE);
    }

    #[test]
    fn invalid_nats_ack_timeout_falls_back_to_default() {
        let env = env_with_secret();
        env.set("GITHUB_NATS_ACK_TIMEOUT_SECS", "not-a-number");

        let config = GithubConfig::from_env(&env);

        assert_eq!(config.nats_ack_timeout, DEFAULT_NATS_ACK_TIMEOUT);
    }
}
