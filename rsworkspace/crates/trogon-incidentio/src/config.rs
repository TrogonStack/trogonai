//! Configuration for the incident.io webhook receiver.

use std::time::Duration;

use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

const DEFAULT_PORT: u16 = 8081;
const DEFAULT_SUBJECT_PREFIX: &str = "incidentio";
const DEFAULT_STREAM_NAME: &str = "INCIDENTIO";
const DEFAULT_STREAM_MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60; // 7 days

/// Configuration for the incident.io webhook server.
///
/// Resolved from environment variables:
///
/// | Variable | Default | Description |
/// |---|---|---|
/// | `INCIDENTIO_WEBHOOK_SECRET` | — | HMAC-SHA256 secret (omit to skip validation) |
/// | `INCIDENTIO_API_TOKEN` | — | Bearer token for the incident.io REST API |
/// | `INCIDENTIO_WEBHOOK_PORT` | `8081` | HTTP listening port |
/// | `INCIDENTIO_SUBJECT_PREFIX` | `incidentio` | NATS subject prefix |
/// | `INCIDENTIO_STREAM_NAME` | `INCIDENTIO` | JetStream stream name |
/// | `INCIDENTIO_STREAM_MAX_AGE_SECS` | `604800` | Max message age in the stream |
pub struct IncidentioConfig {
    pub webhook_secret: Option<String>,
    pub api_token: Option<String>,
    pub port: u16,
    pub subject_prefix: String,
    pub stream_name: String,
    pub stream_max_age: Duration,
    pub nats: NatsConfig,
}

impl IncidentioConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        Self {
            webhook_secret: env.var("INCIDENTIO_WEBHOOK_SECRET").ok(),
            api_token: env.var("INCIDENTIO_API_TOKEN").ok(),
            port: env
                .var("INCIDENTIO_WEBHOOK_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            subject_prefix: env
                .var("INCIDENTIO_SUBJECT_PREFIX")
                .unwrap_or_else(|_| DEFAULT_SUBJECT_PREFIX.to_string()),
            stream_name: env
                .var("INCIDENTIO_STREAM_NAME")
                .unwrap_or_else(|_| DEFAULT_STREAM_NAME.to_string()),
            stream_max_age: Duration::from_secs(
                env.var("INCIDENTIO_STREAM_MAX_AGE_SECS")
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
        let config = IncidentioConfig::from_env(&env);
        assert!(config.webhook_secret.is_none());
        assert!(config.api_token.is_none());
        assert_eq!(config.port, DEFAULT_PORT);
        assert_eq!(config.subject_prefix, "incidentio");
        assert_eq!(config.stream_name, "INCIDENTIO");
        assert_eq!(
            config.stream_max_age,
            Duration::from_secs(7 * 24 * 60 * 60)
        );
    }

    #[test]
    fn reads_all_env_vars() {
        let env = InMemoryEnv::new();
        env.set("INCIDENTIO_WEBHOOK_SECRET", "wh-secret");
        env.set("INCIDENTIO_API_TOKEN", "api-token");
        env.set("INCIDENTIO_WEBHOOK_PORT", "9000");
        env.set("INCIDENTIO_SUBJECT_PREFIX", "iio");
        env.set("INCIDENTIO_STREAM_NAME", "IIO_EVENTS");
        env.set("INCIDENTIO_STREAM_MAX_AGE_SECS", "3600");

        let config = IncidentioConfig::from_env(&env);
        assert_eq!(config.webhook_secret.as_deref(), Some("wh-secret"));
        assert_eq!(config.api_token.as_deref(), Some("api-token"));
        assert_eq!(config.port, 9000);
        assert_eq!(config.subject_prefix, "iio");
        assert_eq!(config.stream_name, "IIO_EVENTS");
        assert_eq!(config.stream_max_age, Duration::from_secs(3600));
    }

    #[test]
    fn invalid_port_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("INCIDENTIO_WEBHOOK_PORT", "not-a-port");
        let config = IncidentioConfig::from_env(&env);
        assert_eq!(config.port, DEFAULT_PORT);
    }

    #[test]
    fn invalid_max_age_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("INCIDENTIO_STREAM_MAX_AGE_SECS", "not-a-number");
        let config = IncidentioConfig::from_env(&env);
        assert_eq!(
            config.stream_max_age,
            Duration::from_secs(DEFAULT_STREAM_MAX_AGE_SECS)
        );
    }
}
