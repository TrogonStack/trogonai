use std::time::Duration;

use trogon_nats::{NatsConfig, NatsToken};
use trogon_std::env::ReadEnv;

use crate::constants::{
    DEFAULT_NATS_ACK_TIMEOUT_MS, DEFAULT_PORT, DEFAULT_STREAM_MAX_AGE_SECS, DEFAULT_STREAM_NAME,
    DEFAULT_SUBJECT_PREFIX, DEFAULT_TIMESTAMP_TOLERANCE_SECS,
};

/// Configuration for the Linear webhook server.
///
/// Resolved from environment variables:
/// - `LINEAR_WEBHOOK_SECRET`: signing secret from Linear's webhook settings (required)
/// - `LINEAR_WEBHOOK_PORT`: HTTP listening port (default: 8080)
/// - `LINEAR_SUBJECT_PREFIX`: NATS subject prefix (default: `linear`)
/// - `LINEAR_STREAM_NAME`: JetStream stream name (default: `LINEAR`)
/// - `LINEAR_STREAM_MAX_AGE_SECS`: max age of messages in the JetStream stream in seconds (default: 604800 / 7 days)
/// - `LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS`: replay-attack window in seconds (default: 60, set to 0 to disable)
/// - `LINEAR_NATS_ACK_TIMEOUT_MS`: how long to wait for a JetStream ACK in milliseconds (default: 10000)
/// - Standard `NATS_*` variables for NATS connection (see `trogon-nats`)
pub struct LinearConfig {
    pub webhook_secret: String,
    pub port: u16,
    pub subject_prefix: NatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: Duration,
    /// How far in the past a `webhookTimestamp` may be before the request is
    /// rejected as a potential replay.  `None` disables the check entirely
    /// (set `LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS=0`).
    pub timestamp_tolerance: Option<Duration>,
    /// How long to wait for a JetStream ACK before declaring it timed out.
    pub nats_ack_timeout: Duration,
    pub nats: NatsConfig,
}

impl LinearConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let tolerance_secs: u64 = env
            .var("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_TIMESTAMP_TOLERANCE_SECS);

        Self {
            webhook_secret: env
                .var("LINEAR_WEBHOOK_SECRET")
                .ok()
                .filter(|s| !s.is_empty())
                .expect("LINEAR_WEBHOOK_SECRET is required"),
            port: env
                .var("LINEAR_WEBHOOK_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            subject_prefix: NatsToken::new(
                env.var("LINEAR_SUBJECT_PREFIX")
                    .unwrap_or_else(|_| DEFAULT_SUBJECT_PREFIX.to_string()),
            )
            .expect("LINEAR_SUBJECT_PREFIX is not a valid NATS token"),
            stream_name: NatsToken::new(
                env.var("LINEAR_STREAM_NAME")
                    .unwrap_or_else(|_| DEFAULT_STREAM_NAME.to_string()),
            )
            .expect("LINEAR_STREAM_NAME is not a valid NATS token"),
            stream_max_age: Duration::from_secs(
                env.var("LINEAR_STREAM_MAX_AGE_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(DEFAULT_STREAM_MAX_AGE_SECS),
            ),
            timestamp_tolerance: (tolerance_secs > 0).then(|| Duration::from_secs(tolerance_secs)),
            nats_ack_timeout: Duration::from_millis(
                env.var("LINEAR_NATS_ACK_TIMEOUT_MS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(DEFAULT_NATS_ACK_TIMEOUT_MS),
            ),
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
        env.set("LINEAR_WEBHOOK_SECRET", "test-secret");
        env
    }

    #[test]
    #[should_panic(expected = "LINEAR_WEBHOOK_SECRET is required")]
    fn panics_when_webhook_secret_missing() {
        let env = InMemoryEnv::new();
        let _config = LinearConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "LINEAR_WEBHOOK_SECRET is required")]
    fn panics_when_webhook_secret_empty() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_WEBHOOK_SECRET", "");
        let _config = LinearConfig::from_env(&env);
    }

    #[test]
    fn defaults_when_only_secret_set() {
        let env = env_with_secret();
        let config = LinearConfig::from_env(&env);

        assert_eq!(config.webhook_secret, "test-secret");
        assert_eq!(config.port, 8080);
        assert_eq!(config.subject_prefix.as_str(), "linear");
        assert_eq!(config.stream_name.as_str(), "LINEAR");
        assert_eq!(config.stream_max_age, Duration::from_secs(7 * 24 * 60 * 60));
        assert_eq!(config.timestamp_tolerance, Some(Duration::from_secs(60)));
        assert_eq!(
            config.nats_ack_timeout,
            Duration::from_millis(DEFAULT_NATS_ACK_TIMEOUT_MS)
        );
    }

    #[test]
    fn reads_all_env_vars() {
        let env = env_with_secret();
        env.set("LINEAR_WEBHOOK_SECRET", "my-secret");
        env.set("LINEAR_WEBHOOK_PORT", "9090");
        env.set("LINEAR_SUBJECT_PREFIX", "lin");
        env.set("LINEAR_STREAM_NAME", "LIN_EVENTS");
        env.set("LINEAR_STREAM_MAX_AGE_SECS", "3600");
        env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "120");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.webhook_secret, "my-secret");
        assert_eq!(config.port, 9090);
        assert_eq!(config.subject_prefix.as_str(), "lin");
        assert_eq!(config.stream_name.as_str(), "LIN_EVENTS");
        assert_eq!(config.stream_max_age, Duration::from_secs(3600));
        assert_eq!(config.timestamp_tolerance, Some(Duration::from_secs(120)));
    }

    #[test]
    #[should_panic(expected = "LINEAR_SUBJECT_PREFIX is not a valid NATS token")]
    fn empty_subject_prefix_panics() {
        let env = env_with_secret();
        env.set("LINEAR_SUBJECT_PREFIX", "");
        let _config = LinearConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "LINEAR_STREAM_NAME is not a valid NATS token")]
    fn empty_stream_name_panics() {
        let env = env_with_secret();
        env.set("LINEAR_STREAM_NAME", "");
        let _config = LinearConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "LINEAR_SUBJECT_PREFIX is not a valid NATS token")]
    fn subject_prefix_with_dots_panics() {
        let env = env_with_secret();
        env.set("LINEAR_SUBJECT_PREFIX", "linear.events");
        let _config = LinearConfig::from_env(&env);
    }

    #[test]
    fn timestamp_tolerance_zero_disables_check() {
        let env = env_with_secret();
        env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "0");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.timestamp_tolerance, None);
    }

    #[test]
    fn invalid_port_falls_back_to_default() {
        let env = env_with_secret();
        env.set("LINEAR_WEBHOOK_PORT", "not-a-number");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.port, 8080);
    }

    #[test]
    fn invalid_max_age_falls_back_to_default() {
        let env = env_with_secret();
        env.set("LINEAR_STREAM_MAX_AGE_SECS", "not-a-number");

        let config = LinearConfig::from_env(&env);

        assert_eq!(
            config.stream_max_age,
            Duration::from_secs(DEFAULT_STREAM_MAX_AGE_SECS)
        );
    }

    #[test]
    fn invalid_tolerance_falls_back_to_default() {
        let env = env_with_secret();
        env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "not-a-number");

        let config = LinearConfig::from_env(&env);

        assert_eq!(
            config.timestamp_tolerance,
            Some(Duration::from_secs(DEFAULT_TIMESTAMP_TOLERANCE_SECS))
        );
    }

    #[test]
    fn port_overflow_falls_back_to_default() {
        let env = env_with_secret();
        env.set("LINEAR_WEBHOOK_PORT", "65536"); // u16::MAX is 65535

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.port, DEFAULT_PORT);
    }

    #[test]
    fn stream_max_age_zero_produces_zero_duration() {
        let env = env_with_secret();
        env.set("LINEAR_STREAM_MAX_AGE_SECS", "0");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.stream_max_age, Duration::from_secs(0));
    }

    #[test]
    fn negative_port_falls_back_to_default() {
        let env = env_with_secret();
        env.set("LINEAR_WEBHOOK_PORT", "-1");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.port, DEFAULT_PORT);
    }

    #[test]
    fn float_port_falls_back_to_default() {
        let env = env_with_secret();
        env.set("LINEAR_WEBHOOK_PORT", "8080.5");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.port, DEFAULT_PORT);
    }

    #[test]
    fn port_with_trailing_chars_falls_back_to_default() {
        let env = env_with_secret();
        env.set("LINEAR_WEBHOOK_PORT", "8080abc");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.port, DEFAULT_PORT);
    }

    #[test]
    fn negative_max_age_falls_back_to_default() {
        let env = env_with_secret();
        env.set("LINEAR_STREAM_MAX_AGE_SECS", "-1");

        let config = LinearConfig::from_env(&env);

        assert_eq!(
            config.stream_max_age,
            Duration::from_secs(DEFAULT_STREAM_MAX_AGE_SECS)
        );
    }

    #[test]
    fn float_max_age_falls_back_to_default() {
        let env = env_with_secret();
        env.set("LINEAR_STREAM_MAX_AGE_SECS", "3600.5");

        let config = LinearConfig::from_env(&env);

        assert_eq!(
            config.stream_max_age,
            Duration::from_secs(DEFAULT_STREAM_MAX_AGE_SECS)
        );
    }

    #[test]
    fn tolerance_secs_one_is_minimum_non_zero() {
        let env = env_with_secret();
        env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "1");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.timestamp_tolerance, Some(Duration::from_secs(1)));
    }

    #[test]
    fn negative_tolerance_falls_back_to_default() {
        let env = env_with_secret();
        env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "-1");

        let config = LinearConfig::from_env(&env);

        assert_eq!(
            config.timestamp_tolerance,
            Some(Duration::from_secs(DEFAULT_TIMESTAMP_TOLERANCE_SECS))
        );
    }

    #[test]
    fn float_tolerance_falls_back_to_default() {
        let env = env_with_secret();
        env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "60.5");

        let config = LinearConfig::from_env(&env);

        assert_eq!(
            config.timestamp_tolerance,
            Some(Duration::from_secs(DEFAULT_TIMESTAMP_TOLERANCE_SECS))
        );
    }

    #[test]
    fn defaults_nats_ack_timeout_to_10_seconds() {
        let env = env_with_secret();
        let config = LinearConfig::from_env(&env);
        assert_eq!(config.nats_ack_timeout, Duration::from_secs(10));
    }

    #[test]
    fn reads_nats_ack_timeout_from_env() {
        let env = env_with_secret();
        env.set("LINEAR_NATS_ACK_TIMEOUT_MS", "500");
        let config = LinearConfig::from_env(&env);
        assert_eq!(config.nats_ack_timeout, Duration::from_millis(500));
    }

    #[test]
    fn invalid_nats_ack_timeout_falls_back_to_default() {
        let env = env_with_secret();
        env.set("LINEAR_NATS_ACK_TIMEOUT_MS", "not-a-number");
        let config = LinearConfig::from_env(&env);
        assert_eq!(
            config.nats_ack_timeout,
            Duration::from_millis(DEFAULT_NATS_ACK_TIMEOUT_MS)
        );
    }

    #[test]
    fn zero_nats_ack_timeout_is_valid() {
        let env = env_with_secret();
        env.set("LINEAR_NATS_ACK_TIMEOUT_MS", "0");
        let config = LinearConfig::from_env(&env);
        assert_eq!(config.nats_ack_timeout, Duration::ZERO);
    }
}
