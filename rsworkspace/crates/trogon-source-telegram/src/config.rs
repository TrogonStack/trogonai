use std::time::Duration;

use bytesize::ByteSize;
use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

use crate::constants::{
    DEFAULT_MAX_BODY_SIZE, DEFAULT_NATS_ACK_TIMEOUT, DEFAULT_PORT, DEFAULT_STREAM_MAX_AGE,
    DEFAULT_STREAM_NAME, DEFAULT_SUBJECT_PREFIX,
};

/// Configuration for the Telegram webhook source.
///
/// Resolved from environment variables:
/// - `TELEGRAM_WEBHOOK_SECRET`: secret token configured via `setWebhook` (**required**)
/// - `TELEGRAM_SOURCE_PORT`: HTTP listening port (default: 8080)
/// - `TELEGRAM_SUBJECT_PREFIX`: NATS subject prefix (default: `telegram`)
/// - `TELEGRAM_STREAM_NAME`: JetStream stream name (default: `TELEGRAM`)
/// - `TELEGRAM_STREAM_MAX_AGE_SECS`: max age in seconds (default: 604800 / 7 days)
/// - `TELEGRAM_NATS_ACK_TIMEOUT_SECS`: NATS ack timeout in seconds (default: 10)
/// - `TELEGRAM_MAX_BODY_SIZE`: maximum body size in bytes (default: 10 MB)
/// - Standard `NATS_*` variables for NATS connection (see `trogon-nats`)
pub struct TelegramSourceConfig {
    pub webhook_secret: String,
    pub port: u16,
    pub subject_prefix: String,
    pub stream_name: String,
    pub stream_max_age: Duration,
    pub nats_ack_timeout: Duration,
    pub max_body_size: ByteSize,
    pub nats: NatsConfig,
}

impl TelegramSourceConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        Self {
            webhook_secret: env
                .var("TELEGRAM_WEBHOOK_SECRET")
                .ok()
                .filter(|s| !s.is_empty())
                .expect("TELEGRAM_WEBHOOK_SECRET is required"),
            port: env
                .var("TELEGRAM_SOURCE_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            subject_prefix: env
                .var("TELEGRAM_SUBJECT_PREFIX")
                .unwrap_or_else(|_| DEFAULT_SUBJECT_PREFIX.to_string()),
            stream_name: env
                .var("TELEGRAM_STREAM_NAME")
                .unwrap_or_else(|_| DEFAULT_STREAM_NAME.to_string()),
            stream_max_age: env
                .var("TELEGRAM_STREAM_MAX_AGE_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_STREAM_MAX_AGE),
            nats_ack_timeout: env
                .var("TELEGRAM_NATS_ACK_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_NATS_ACK_TIMEOUT),
            max_body_size: env
                .var("TELEGRAM_MAX_BODY_SIZE")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .map(ByteSize)
                .unwrap_or(DEFAULT_MAX_BODY_SIZE),
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
        env.set("TELEGRAM_WEBHOOK_SECRET", "test-secret");
        env
    }

    #[test]
    fn defaults_with_required_secret() {
        let env = env_with_secret();
        let config = TelegramSourceConfig::from_env(&env);

        assert_eq!(config.webhook_secret, "test-secret");
        assert_eq!(config.port, 8080);
        assert_eq!(config.subject_prefix, "telegram");
        assert_eq!(config.stream_name, "TELEGRAM");
        assert_eq!(
            config.stream_max_age,
            Duration::from_secs(7 * 24 * 60 * 60)
        );
        assert_eq!(config.nats_ack_timeout, Duration::from_secs(10));
        assert_eq!(config.max_body_size, ByteSize::mib(10));
    }

    #[test]
    fn reads_all_env_vars() {
        let env = InMemoryEnv::new();
        env.set("TELEGRAM_WEBHOOK_SECRET", "my-secret");
        env.set("TELEGRAM_SOURCE_PORT", "9090");
        env.set("TELEGRAM_SUBJECT_PREFIX", "tg");
        env.set("TELEGRAM_STREAM_NAME", "TG_EVENTS");
        env.set("TELEGRAM_STREAM_MAX_AGE_SECS", "3600");
        env.set("TELEGRAM_NATS_ACK_TIMEOUT_SECS", "30");
        env.set("TELEGRAM_MAX_BODY_SIZE", "1048576");

        let config = TelegramSourceConfig::from_env(&env);

        assert_eq!(config.webhook_secret, "my-secret");
        assert_eq!(config.port, 9090);
        assert_eq!(config.subject_prefix, "tg");
        assert_eq!(config.stream_name, "TG_EVENTS");
        assert_eq!(config.stream_max_age, Duration::from_secs(3600));
        assert_eq!(config.nats_ack_timeout, Duration::from_secs(30));
        assert_eq!(config.max_body_size, ByteSize::mib(1));
    }

    #[test]
    #[should_panic(expected = "TELEGRAM_WEBHOOK_SECRET is required")]
    fn missing_webhook_secret_panics() {
        let env = InMemoryEnv::new();
        TelegramSourceConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "TELEGRAM_WEBHOOK_SECRET is required")]
    fn empty_webhook_secret_panics() {
        let env = InMemoryEnv::new();
        env.set("TELEGRAM_WEBHOOK_SECRET", "");
        TelegramSourceConfig::from_env(&env);
    }

    #[test]
    fn invalid_port_falls_back_to_default() {
        let env = env_with_secret();
        env.set("TELEGRAM_SOURCE_PORT", "not-a-number");
        let config = TelegramSourceConfig::from_env(&env);
        assert_eq!(config.port, 8080);
    }

    #[test]
    fn invalid_max_age_falls_back_to_default() {
        let env = env_with_secret();
        env.set("TELEGRAM_STREAM_MAX_AGE_SECS", "not-a-number");
        let config = TelegramSourceConfig::from_env(&env);
        assert_eq!(config.stream_max_age, DEFAULT_STREAM_MAX_AGE);
    }

    #[test]
    fn invalid_nats_ack_timeout_falls_back_to_default() {
        let env = env_with_secret();
        env.set("TELEGRAM_NATS_ACK_TIMEOUT_SECS", "not-a-number");
        let config = TelegramSourceConfig::from_env(&env);
        assert_eq!(config.nats_ack_timeout, DEFAULT_NATS_ACK_TIMEOUT);
    }

    #[test]
    fn invalid_max_body_size_falls_back_to_default() {
        let env = env_with_secret();
        env.set("TELEGRAM_MAX_BODY_SIZE", "not-a-number");
        let config = TelegramSourceConfig::from_env(&env);
        assert_eq!(config.max_body_size, DEFAULT_MAX_BODY_SIZE);
    }
}
