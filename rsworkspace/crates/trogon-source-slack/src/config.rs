use std::time::Duration;

use trogon_nats::{NatsConfig, NatsToken};
use trogon_std::env::ReadEnv;

use crate::constants::{
    DEFAULT_NATS_ACK_TIMEOUT, DEFAULT_PORT, DEFAULT_STREAM_MAX_AGE, DEFAULT_STREAM_NAME,
    DEFAULT_SUBJECT_PREFIX, DEFAULT_TIMESTAMP_MAX_DRIFT_SECS,
};

/// Configuration for the Slack Events API webhook server.
///
/// Resolved from environment variables:
/// - `SLACK_SIGNING_SECRET`: Slack app signing secret (**required**)
/// - `SLACK_WEBHOOK_PORT`: HTTP listening port (default: 3000)
/// - `SLACK_SUBJECT_PREFIX`: NATS subject prefix (default: `slack`)
/// - `SLACK_STREAM_NAME`: JetStream stream name (default: `SLACK`)
/// - `SLACK_STREAM_MAX_AGE_SECS`: max age of messages in the JetStream stream in seconds (default: 604800 / 7 days)
/// - `SLACK_NATS_ACK_TIMEOUT_SECS`: NATS ack timeout in seconds (default: 10)
/// - `SLACK_MAX_BODY_SIZE`: maximum webhook body size in bytes (default: 1048576 / 1 MB)
/// - `SLACK_TIMESTAMP_MAX_DRIFT_SECS`: max allowed clock drift for request timestamps in seconds (default: 300 / 5 min)
/// - Standard `NATS_*` variables for NATS connection (see `trogon-nats`)
pub struct SlackConfig {
    pub signing_secret: String,
    pub port: u16,
    pub subject_prefix: NatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: Duration,
    pub nats_ack_timeout: Duration,
    pub timestamp_max_drift: Duration,
    pub nats: NatsConfig,
}

impl SlackConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        Self {
            signing_secret: env
                .var("SLACK_SIGNING_SECRET")
                .ok()
                .filter(|s| !s.is_empty())
                .expect("SLACK_SIGNING_SECRET is required"),
            port: env
                .var("SLACK_WEBHOOK_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            subject_prefix: NatsToken::new(
                env.var("SLACK_SUBJECT_PREFIX")
                    .unwrap_or_else(|_| DEFAULT_SUBJECT_PREFIX.to_string()),
            )
            .expect("SLACK_SUBJECT_PREFIX is not a valid NATS token"),
            stream_name: NatsToken::new(
                env.var("SLACK_STREAM_NAME")
                    .unwrap_or_else(|_| DEFAULT_STREAM_NAME.to_string()),
            )
            .expect("SLACK_STREAM_NAME is not a valid NATS token"),
            stream_max_age: env
                .var("SLACK_STREAM_MAX_AGE_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_STREAM_MAX_AGE),
            nats_ack_timeout: env
                .var("SLACK_NATS_ACK_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|&v| v > 0)
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_NATS_ACK_TIMEOUT),
            timestamp_max_drift: env
                .var("SLACK_TIMESTAMP_MAX_DRIFT_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|&v| v > 0)
                .map(Duration::from_secs)
                .unwrap_or(Duration::from_secs(DEFAULT_TIMESTAMP_MAX_DRIFT_SECS)),
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
        env.set("SLACK_SIGNING_SECRET", "test-secret");
        env
    }

    #[test]
    fn defaults_with_required_secret() {
        let env = env_with_secret();
        let config = SlackConfig::from_env(&env);

        assert_eq!(config.signing_secret, "test-secret");
        assert_eq!(config.port, 3000);
        assert_eq!(config.subject_prefix.as_str(), "slack");
        assert_eq!(config.stream_name.as_str(), "SLACK");
        assert_eq!(config.stream_max_age, Duration::from_secs(7 * 24 * 60 * 60));
        assert_eq!(config.nats_ack_timeout, Duration::from_secs(10));
        assert_eq!(config.timestamp_max_drift, Duration::from_secs(300));
    }

    #[test]
    fn reads_all_env_vars() {
        let env = InMemoryEnv::new();
        env.set("SLACK_SIGNING_SECRET", "my-secret");
        env.set("SLACK_WEBHOOK_PORT", "9090");
        env.set("SLACK_SUBJECT_PREFIX", "slk");
        env.set("SLACK_STREAM_NAME", "SLK_EVENTS");
        env.set("SLACK_STREAM_MAX_AGE_SECS", "3600");
        env.set("SLACK_NATS_ACK_TIMEOUT_SECS", "30");
        env.set("SLACK_TIMESTAMP_MAX_DRIFT_SECS", "60");

        let config = SlackConfig::from_env(&env);

        assert_eq!(config.signing_secret, "my-secret");
        assert_eq!(config.port, 9090);
        assert_eq!(config.subject_prefix.as_str(), "slk");
        assert_eq!(config.stream_name.as_str(), "SLK_EVENTS");
        assert_eq!(config.stream_max_age, Duration::from_secs(3600));
        assert_eq!(config.nats_ack_timeout, Duration::from_secs(30));
        assert_eq!(config.timestamp_max_drift, Duration::from_secs(60));
    }

    #[test]
    #[should_panic(expected = "SLACK_SIGNING_SECRET is required")]
    fn missing_signing_secret_panics() {
        let env = InMemoryEnv::new();
        SlackConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "SLACK_SIGNING_SECRET is required")]
    fn empty_signing_secret_panics() {
        let env = InMemoryEnv::new();
        env.set("SLACK_SIGNING_SECRET", "");
        SlackConfig::from_env(&env);
    }

    #[test]
    fn invalid_port_falls_back_to_default() {
        let env = env_with_secret();
        env.set("SLACK_WEBHOOK_PORT", "not-a-number");
        let config = SlackConfig::from_env(&env);
        assert_eq!(config.port, 3000);
    }

    #[test]
    fn invalid_max_age_falls_back_to_default() {
        let env = env_with_secret();
        env.set("SLACK_STREAM_MAX_AGE_SECS", "not-a-number");
        let config = SlackConfig::from_env(&env);
        assert_eq!(config.stream_max_age, DEFAULT_STREAM_MAX_AGE);
    }

    #[test]
    fn invalid_nats_ack_timeout_falls_back_to_default() {
        let env = env_with_secret();
        env.set("SLACK_NATS_ACK_TIMEOUT_SECS", "not-a-number");
        let config = SlackConfig::from_env(&env);
        assert_eq!(config.nats_ack_timeout, DEFAULT_NATS_ACK_TIMEOUT);
    }

    #[test]
    fn invalid_timestamp_drift_falls_back_to_default() {
        let env = env_with_secret();
        env.set("SLACK_TIMESTAMP_MAX_DRIFT_SECS", "not-a-number");
        let config = SlackConfig::from_env(&env);
        assert_eq!(config.timestamp_max_drift, Duration::from_secs(300));
    }

    #[test]
    #[should_panic(expected = "SLACK_SUBJECT_PREFIX is not a valid NATS token")]
    fn empty_subject_prefix_panics() {
        let env = env_with_secret();
        env.set("SLACK_SUBJECT_PREFIX", "");
        SlackConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "SLACK_SUBJECT_PREFIX is not a valid NATS token")]
    fn wildcard_subject_prefix_panics() {
        let env = env_with_secret();
        env.set("SLACK_SUBJECT_PREFIX", "slack.>");
        SlackConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "SLACK_STREAM_NAME is not a valid NATS token")]
    fn empty_stream_name_panics() {
        let env = env_with_secret();
        env.set("SLACK_STREAM_NAME", "");
        SlackConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "SLACK_STREAM_NAME is not a valid NATS token")]
    fn whitespace_stream_name_panics() {
        let env = env_with_secret();
        env.set("SLACK_STREAM_NAME", "SLACK EVENTS");
        SlackConfig::from_env(&env);
    }

    #[test]
    fn zero_nats_ack_timeout_falls_back_to_default() {
        let env = env_with_secret();
        env.set("SLACK_NATS_ACK_TIMEOUT_SECS", "0");
        let config = SlackConfig::from_env(&env);
        assert_eq!(config.nats_ack_timeout, DEFAULT_NATS_ACK_TIMEOUT);
    }

    #[test]
    fn zero_timestamp_drift_falls_back_to_default() {
        let env = env_with_secret();
        env.set("SLACK_TIMESTAMP_MAX_DRIFT_SECS", "0");
        let config = SlackConfig::from_env(&env);
        assert_eq!(
            config.timestamp_max_drift,
            Duration::from_secs(DEFAULT_TIMESTAMP_MAX_DRIFT_SECS)
        );
    }
}
