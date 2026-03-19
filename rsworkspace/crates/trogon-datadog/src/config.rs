use std::time::Duration;

use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

const DEFAULT_PORT: u16 = 8080;
const DEFAULT_SUBJECT_PREFIX: &str = "datadog";
const DEFAULT_STREAM_NAME: &str = "DATADOG";
const DEFAULT_STREAM_MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60; // 7 days

/// Configuration for the Datadog webhook server.
///
/// Resolved from environment variables:
/// - `DATADOG_WEBHOOK_SECRET`: HMAC-SHA256 secret configured in Datadog (required for signature validation)
/// - `DATADOG_WEBHOOK_PORT`: HTTP listening port (default: 8080)
/// - `DATADOG_SUBJECT_PREFIX`: NATS subject prefix (default: `datadog`)
/// - `DATADOG_STREAM_NAME`: JetStream stream name (default: `DATADOG`)
/// - `DATADOG_STREAM_MAX_AGE_SECS`: max age of messages in the JetStream stream in seconds (default: 604800 / 7 days)
/// - Standard `NATS_*` variables for NATS connection (see `trogon-nats`)
pub struct DatadogConfig {
    pub webhook_secret: Option<String>,
    pub port: u16,
    pub subject_prefix: String,
    pub stream_name: String,
    pub stream_max_age: Duration,
    pub nats: NatsConfig,
}

impl DatadogConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        Self {
            webhook_secret: env.var("DATADOG_WEBHOOK_SECRET").ok(),
            port: env
                .var("DATADOG_WEBHOOK_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            subject_prefix: env
                .var("DATADOG_SUBJECT_PREFIX")
                .unwrap_or_else(|_| DEFAULT_SUBJECT_PREFIX.to_string()),
            stream_name: env
                .var("DATADOG_STREAM_NAME")
                .unwrap_or_else(|_| DEFAULT_STREAM_NAME.to_string()),
            stream_max_age: Duration::from_secs(
                env.var("DATADOG_STREAM_MAX_AGE_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(DEFAULT_STREAM_MAX_AGE_SECS),
            ),
            nats: NatsConfig::from_env(env),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;

    #[test]
    fn defaults_when_no_env_vars() {
        let env = InMemoryEnv::new();
        let config = DatadogConfig::from_env(&env);

        assert!(config.webhook_secret.is_none());
        assert_eq!(config.port, 8080);
        assert_eq!(config.subject_prefix, "datadog");
        assert_eq!(config.stream_name, "DATADOG");
        assert_eq!(config.stream_max_age, Duration::from_secs(7 * 24 * 60 * 60));
    }

    #[test]
    fn reads_all_env_vars() {
        let env = InMemoryEnv::new();
        env.set("DATADOG_WEBHOOK_SECRET", "my-secret");
        env.set("DATADOG_WEBHOOK_PORT", "9090");
        env.set("DATADOG_SUBJECT_PREFIX", "dd");
        env.set("DATADOG_STREAM_NAME", "DD_EVENTS");
        env.set("DATADOG_STREAM_MAX_AGE_SECS", "3600");

        let config = DatadogConfig::from_env(&env);

        assert_eq!(config.webhook_secret.as_deref(), Some("my-secret"));
        assert_eq!(config.port, 9090);
        assert_eq!(config.subject_prefix, "dd");
        assert_eq!(config.stream_name, "DD_EVENTS");
        assert_eq!(config.stream_max_age, Duration::from_secs(3600));
    }

    #[test]
    fn invalid_port_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("DATADOG_WEBHOOK_PORT", "not-a-number");

        let config = DatadogConfig::from_env(&env);

        assert_eq!(config.port, DEFAULT_PORT);
    }

    #[test]
    fn invalid_max_age_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("DATADOG_STREAM_MAX_AGE_SECS", "not-a-number");

        let config = DatadogConfig::from_env(&env);

        assert_eq!(config.stream_max_age, Duration::from_secs(DEFAULT_STREAM_MAX_AGE_SECS));
    }
}
