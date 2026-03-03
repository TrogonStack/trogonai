use std::time::Duration;

use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

const DEFAULT_PORT: u16 = 8080;
const DEFAULT_SUBJECT_PREFIX: &str = "linear";
const DEFAULT_STREAM_NAME: &str = "LINEAR";
const DEFAULT_STREAM_MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60; // 7 days
/// Default replay-attack tolerance: 60 seconds (as recommended by Linear).
pub const DEFAULT_TIMESTAMP_TOLERANCE_SECS: u64 = 60;
/// Default JetStream ACK timeout: 10 seconds.
pub const DEFAULT_NATS_ACK_TIMEOUT_MS: u64 = 10_000;
/// Default JetStream stream-operation timeout: 10 seconds.
pub const DEFAULT_NATS_STREAM_OP_TIMEOUT_MS: u64 = 10_000;

/// Configuration for the Linear webhook server.
///
/// Resolved from environment variables:
/// - `LINEAR_WEBHOOK_SECRET`: HMAC-SHA256 secret configured in Linear (required for signature validation)
/// - `LINEAR_WEBHOOK_PORT`: HTTP listening port (default: 8080)
/// - `LINEAR_SUBJECT_PREFIX`: NATS subject prefix (default: `linear`)
/// - `LINEAR_STREAM_NAME`: JetStream stream name (default: `LINEAR`)
/// - `LINEAR_STREAM_MAX_AGE_SECS`: max age of messages in the JetStream stream in seconds (default: 604800 / 7 days)
/// - `LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS`: replay-attack window in seconds (default: 60, set to 0 to disable)
/// - `LINEAR_NATS_ACK_TIMEOUT_MS`: how long to wait for a JetStream ACK in milliseconds (default: 10000)
/// - `LINEAR_NATS_STREAM_OP_TIMEOUT_MS`: how long to wait for stream operations in milliseconds (default: 10000)
/// - Standard `NATS_*` variables for NATS connection (see `trogon-nats`)
pub struct LinearConfig {
    pub webhook_secret: Option<String>,
    pub port: u16,
    pub subject_prefix: String,
    pub stream_name: String,
    pub stream_max_age: Duration,
    /// How far in the past a `webhookTimestamp` may be before the request is
    /// rejected as a potential replay.  `None` disables the check entirely
    /// (set `LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS=0`).
    pub timestamp_tolerance: Option<Duration>,
    /// How long to wait for a JetStream ACK before declaring it timed out.
    pub nats_ack_timeout: Duration,
    /// How long to wait for JetStream stream operations (e.g. `get_or_create_stream`).
    pub nats_stream_op_timeout: Duration,
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
            webhook_secret: env.var("LINEAR_WEBHOOK_SECRET").ok(),
            port: env
                .var("LINEAR_WEBHOOK_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            subject_prefix: env
                .var("LINEAR_SUBJECT_PREFIX")
                .unwrap_or_else(|_| DEFAULT_SUBJECT_PREFIX.to_string()),
            stream_name: env
                .var("LINEAR_STREAM_NAME")
                .unwrap_or_else(|_| DEFAULT_STREAM_NAME.to_string()),
            stream_max_age: Duration::from_secs(
                env.var("LINEAR_STREAM_MAX_AGE_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(DEFAULT_STREAM_MAX_AGE_SECS),
            ),
            timestamp_tolerance: (tolerance_secs > 0)
                .then(|| Duration::from_secs(tolerance_secs)),
            nats_ack_timeout: Duration::from_millis(
                env.var("LINEAR_NATS_ACK_TIMEOUT_MS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(DEFAULT_NATS_ACK_TIMEOUT_MS),
            ),
            nats_stream_op_timeout: Duration::from_millis(
                env.var("LINEAR_NATS_STREAM_OP_TIMEOUT_MS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(DEFAULT_NATS_STREAM_OP_TIMEOUT_MS),
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
        let config = LinearConfig::from_env(&env);

        assert!(config.webhook_secret.is_none());
        assert_eq!(config.port, 8080);
        assert_eq!(config.subject_prefix, "linear");
        assert_eq!(config.stream_name, "LINEAR");
        assert_eq!(config.stream_max_age, Duration::from_secs(7 * 24 * 60 * 60));
        assert_eq!(config.timestamp_tolerance, Some(Duration::from_secs(60)));
        assert_eq!(config.nats_ack_timeout, Duration::from_millis(DEFAULT_NATS_ACK_TIMEOUT_MS));
        assert_eq!(config.nats_stream_op_timeout, Duration::from_millis(DEFAULT_NATS_STREAM_OP_TIMEOUT_MS));
    }

    #[test]
    fn reads_all_env_vars() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_WEBHOOK_SECRET", "my-secret");
        env.set("LINEAR_WEBHOOK_PORT", "9090");
        env.set("LINEAR_SUBJECT_PREFIX", "lin");
        env.set("LINEAR_STREAM_NAME", "LIN_EVENTS");
        env.set("LINEAR_STREAM_MAX_AGE_SECS", "3600");
        env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "120");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.webhook_secret.as_deref(), Some("my-secret"));
        assert_eq!(config.port, 9090);
        assert_eq!(config.subject_prefix, "lin");
        assert_eq!(config.stream_name, "LIN_EVENTS");
        assert_eq!(config.stream_max_age, Duration::from_secs(3600));
        assert_eq!(config.timestamp_tolerance, Some(Duration::from_secs(120)));
    }

    #[test]
    fn timestamp_tolerance_zero_disables_check() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "0");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.timestamp_tolerance, None);
    }

    #[test]
    fn invalid_port_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_WEBHOOK_PORT", "not-a-number");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.port, 8080);
    }

    #[test]
    fn invalid_max_age_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_STREAM_MAX_AGE_SECS", "not-a-number");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.stream_max_age, Duration::from_secs(DEFAULT_STREAM_MAX_AGE_SECS));
    }

    #[test]
    fn invalid_tolerance_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "not-a-number");

        let config = LinearConfig::from_env(&env);

        assert_eq!(
            config.timestamp_tolerance,
            Some(Duration::from_secs(DEFAULT_TIMESTAMP_TOLERANCE_SECS))
        );
    }

    #[test]
    fn port_overflow_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_WEBHOOK_PORT", "65536"); // u16::MAX is 65535

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.port, DEFAULT_PORT);
    }

    #[test]
    fn stream_max_age_zero_produces_zero_duration() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_STREAM_MAX_AGE_SECS", "0");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.stream_max_age, Duration::from_secs(0));
    }

    // ── Parser edge cases: port ───────────────────────────────────────────────

    #[test]
    fn negative_port_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_WEBHOOK_PORT", "-1");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.port, DEFAULT_PORT);
    }

    #[test]
    fn float_port_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_WEBHOOK_PORT", "8080.5");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.port, DEFAULT_PORT);
    }

    #[test]
    fn port_with_trailing_chars_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_WEBHOOK_PORT", "8080abc");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.port, DEFAULT_PORT);
    }

    // ── Parser edge cases: max_age ────────────────────────────────────────────

    #[test]
    fn negative_max_age_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_STREAM_MAX_AGE_SECS", "-1");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.stream_max_age, Duration::from_secs(DEFAULT_STREAM_MAX_AGE_SECS));
    }

    #[test]
    fn float_max_age_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_STREAM_MAX_AGE_SECS", "3600.5");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.stream_max_age, Duration::from_secs(DEFAULT_STREAM_MAX_AGE_SECS));
    }

    // ── Parser edge cases: timestamp tolerance ────────────────────────────────

    #[test]
    fn tolerance_secs_one_is_minimum_non_zero() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "1");

        let config = LinearConfig::from_env(&env);

        assert_eq!(config.timestamp_tolerance, Some(Duration::from_secs(1)));
    }

    #[test]
    fn negative_tolerance_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "-1");

        let config = LinearConfig::from_env(&env);

        assert_eq!(
            config.timestamp_tolerance,
            Some(Duration::from_secs(DEFAULT_TIMESTAMP_TOLERANCE_SECS))
        );
    }

    #[test]
    fn float_tolerance_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "60.5");

        let config = LinearConfig::from_env(&env);

        assert_eq!(
            config.timestamp_tolerance,
            Some(Duration::from_secs(DEFAULT_TIMESTAMP_TOLERANCE_SECS))
        );
    }

    // ── Parser edge cases: NATS ACK / stream-op timeouts ─────────────────────

    #[test]
    fn defaults_nats_ack_timeout_to_10_seconds() {
        let env = InMemoryEnv::new();
        let config = LinearConfig::from_env(&env);
        assert_eq!(config.nats_ack_timeout, Duration::from_secs(10));
    }

    #[test]
    fn reads_nats_ack_timeout_from_env() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_NATS_ACK_TIMEOUT_MS", "500");
        let config = LinearConfig::from_env(&env);
        assert_eq!(config.nats_ack_timeout, Duration::from_millis(500));
    }

    #[test]
    fn invalid_nats_ack_timeout_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_NATS_ACK_TIMEOUT_MS", "not-a-number");
        let config = LinearConfig::from_env(&env);
        assert_eq!(config.nats_ack_timeout, Duration::from_millis(DEFAULT_NATS_ACK_TIMEOUT_MS));
    }

    #[test]
    fn zero_nats_ack_timeout_is_valid() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_NATS_ACK_TIMEOUT_MS", "0");
        let config = LinearConfig::from_env(&env);
        assert_eq!(config.nats_ack_timeout, Duration::ZERO);
    }

    #[test]
    fn defaults_nats_stream_op_timeout_to_10_seconds() {
        let env = InMemoryEnv::new();
        let config = LinearConfig::from_env(&env);
        assert_eq!(config.nats_stream_op_timeout, Duration::from_secs(10));
    }

    #[test]
    fn reads_nats_stream_op_timeout_from_env() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_NATS_STREAM_OP_TIMEOUT_MS", "250");
        let config = LinearConfig::from_env(&env);
        assert_eq!(config.nats_stream_op_timeout, Duration::from_millis(250));
    }

    #[test]
    fn invalid_nats_stream_op_timeout_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("LINEAR_NATS_STREAM_OP_TIMEOUT_MS", "bad");
        let config = LinearConfig::from_env(&env);
        assert_eq!(
            config.nats_stream_op_timeout,
            Duration::from_millis(DEFAULT_NATS_STREAM_OP_TIMEOUT_MS)
        );
    }
}
