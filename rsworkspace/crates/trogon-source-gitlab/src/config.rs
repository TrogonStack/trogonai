use std::time::Duration;

use bytesize::ByteSize;
use trogon_nats::NatsConfig;
use trogon_nats::NatsToken;
use trogon_std::env::ReadEnv;

use crate::constants::{
    DEFAULT_MAX_BODY_SIZE, DEFAULT_NATS_ACK_TIMEOUT_MS, DEFAULT_PORT, DEFAULT_STREAM_MAX_AGE,
    DEFAULT_STREAM_NAME, DEFAULT_SUBJECT_PREFIX,
};
use crate::webhook_secret::WebhookSecret;

pub struct GitlabConfig {
    pub webhook_secret: WebhookSecret,
    pub port: u16,
    pub subject_prefix: NatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: Duration,
    pub nats_ack_timeout: Duration,
    pub max_body_size: ByteSize,
    pub nats: NatsConfig,
}

impl GitlabConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        Self {
            webhook_secret: WebhookSecret::new(
                env.var("GITLAB_WEBHOOK_SECRET").unwrap_or_default(),
            )
            .expect("GITLAB_WEBHOOK_SECRET is required"),
            port: env
                .var("GITLAB_WEBHOOK_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            subject_prefix: NatsToken::new(
                env.var("GITLAB_SUBJECT_PREFIX")
                    .unwrap_or_else(|_| DEFAULT_SUBJECT_PREFIX.to_string()),
            )
            .expect("GITLAB_SUBJECT_PREFIX is not a valid NATS token"),
            stream_name: NatsToken::new(
                env.var("GITLAB_STREAM_NAME")
                    .unwrap_or_else(|_| DEFAULT_STREAM_NAME.to_string()),
            )
            .expect("GITLAB_STREAM_NAME is not a valid NATS token"),
            stream_max_age: env
                .var("GITLAB_STREAM_MAX_AGE_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_STREAM_MAX_AGE),
            nats_ack_timeout: env
                .var("GITLAB_NATS_ACK_TIMEOUT_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(Duration::from_millis)
                .unwrap_or(Duration::from_millis(DEFAULT_NATS_ACK_TIMEOUT_MS)),
            max_body_size: env
                .var("GITLAB_MAX_BODY_SIZE")
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
        env.set("GITLAB_WEBHOOK_SECRET", "test-secret");
        env
    }

    #[test]
    fn defaults_with_required_secret() {
        let env = env_with_secret();
        let config = GitlabConfig::from_env(&env);

        assert_eq!(config.webhook_secret.as_str(), "test-secret");
        assert_eq!(config.port, 8080);
        assert_eq!(config.subject_prefix.as_str(), "gitlab");
        assert_eq!(config.stream_name.as_str(), "GITLAB");
        assert_eq!(config.stream_max_age, Duration::from_secs(7 * 24 * 60 * 60));
        assert_eq!(config.nats_ack_timeout, Duration::from_millis(10_000));
        assert_eq!(config.max_body_size, ByteSize::mib(25));
    }

    #[test]
    fn reads_all_env_vars() {
        let env = InMemoryEnv::new();
        env.set("GITLAB_WEBHOOK_SECRET", "my-secret");
        env.set("GITLAB_WEBHOOK_PORT", "9090");
        env.set("GITLAB_SUBJECT_PREFIX", "gl");
        env.set("GITLAB_STREAM_NAME", "GL_EVENTS");
        env.set("GITLAB_STREAM_MAX_AGE_SECS", "3600");
        env.set("GITLAB_NATS_ACK_TIMEOUT_MS", "5000");
        env.set("GITLAB_MAX_BODY_SIZE", "1048576");

        let config = GitlabConfig::from_env(&env);

        assert_eq!(config.webhook_secret.as_str(), "my-secret");
        assert_eq!(config.port, 9090);
        assert_eq!(config.subject_prefix.as_str(), "gl");
        assert_eq!(config.stream_name.as_str(), "GL_EVENTS");
        assert_eq!(config.stream_max_age, Duration::from_secs(3600));
        assert_eq!(config.nats_ack_timeout, Duration::from_millis(5000));
        assert_eq!(config.max_body_size, ByteSize::mib(1));
    }

    #[test]
    #[should_panic(expected = "GITLAB_WEBHOOK_SECRET is required")]
    fn missing_webhook_secret_panics() {
        let env = InMemoryEnv::new();
        GitlabConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "GITLAB_WEBHOOK_SECRET is required")]
    fn empty_webhook_secret_panics() {
        let env = InMemoryEnv::new();
        env.set("GITLAB_WEBHOOK_SECRET", "");
        GitlabConfig::from_env(&env);
    }

    #[test]
    fn invalid_port_falls_back_to_default() {
        let env = env_with_secret();
        env.set("GITLAB_WEBHOOK_PORT", "not-a-number");

        let config = GitlabConfig::from_env(&env);

        assert_eq!(config.port, 8080);
    }

    #[test]
    fn invalid_max_age_falls_back_to_default() {
        let env = env_with_secret();
        env.set("GITLAB_STREAM_MAX_AGE_SECS", "not-a-number");

        let config = GitlabConfig::from_env(&env);

        assert_eq!(config.stream_max_age, DEFAULT_STREAM_MAX_AGE);
    }

    #[test]
    fn invalid_nats_ack_timeout_falls_back_to_default() {
        let env = env_with_secret();
        env.set("GITLAB_NATS_ACK_TIMEOUT_MS", "not-a-number");

        let config = GitlabConfig::from_env(&env);

        assert_eq!(
            config.nats_ack_timeout,
            Duration::from_millis(DEFAULT_NATS_ACK_TIMEOUT_MS)
        );
    }

    #[test]
    fn invalid_max_body_size_falls_back_to_default() {
        let env = env_with_secret();
        env.set("GITLAB_MAX_BODY_SIZE", "not-a-number");

        let config = GitlabConfig::from_env(&env);

        assert_eq!(config.max_body_size, DEFAULT_MAX_BODY_SIZE);
    }

    #[test]
    #[should_panic(expected = "GITLAB_SUBJECT_PREFIX is not a valid NATS token")]
    fn invalid_subject_prefix_panics() {
        let env = env_with_secret();
        env.set("GITLAB_SUBJECT_PREFIX", "has.dot");
        GitlabConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "GITLAB_STREAM_NAME is not a valid NATS token")]
    fn invalid_stream_name_panics() {
        let env = env_with_secret();
        env.set("GITLAB_STREAM_NAME", "has space");
        GitlabConfig::from_env(&env);
    }
}
