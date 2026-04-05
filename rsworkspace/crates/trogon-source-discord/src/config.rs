use std::time::Duration;

use bytesize::ByteSize;
use ed25519_dalek::VerifyingKey;
use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

use crate::constants::{
    DEFAULT_MAX_BODY_SIZE, DEFAULT_NATS_ACK_TIMEOUT, DEFAULT_PORT, DEFAULT_STREAM_MAX_AGE,
    DEFAULT_STREAM_NAME, DEFAULT_SUBJECT_PREFIX,
};
use crate::signature;

pub struct DiscordConfig {
    pub public_key: VerifyingKey,
    pub port: u16,
    pub subject_prefix: String,
    pub stream_name: String,
    pub stream_max_age: Duration,
    pub nats_ack_timeout: Duration,
    pub max_body_size: ByteSize,
    pub nats: NatsConfig,
}

impl DiscordConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let public_key_hex = env
            .var("DISCORD_PUBLIC_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .expect("DISCORD_PUBLIC_KEY is required");

        let public_key = signature::parse_public_key(&public_key_hex)
            .expect("DISCORD_PUBLIC_KEY must be a valid hex-encoded Ed25519 public key");

        Self {
            public_key,
            port: env
                .var("DISCORD_WEBHOOK_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            subject_prefix: env
                .var("DISCORD_SUBJECT_PREFIX")
                .unwrap_or_else(|_| DEFAULT_SUBJECT_PREFIX.to_string()),
            stream_name: env
                .var("DISCORD_STREAM_NAME")
                .unwrap_or_else(|_| DEFAULT_STREAM_NAME.to_string()),
            stream_max_age: env
                .var("DISCORD_STREAM_MAX_AGE_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_STREAM_MAX_AGE),
            nats_ack_timeout: env
                .var("DISCORD_NATS_ACK_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_NATS_ACK_TIMEOUT),
            max_body_size: env
                .var("DISCORD_MAX_BODY_SIZE")
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
    use ed25519_dalek::SigningKey;
    use trogon_std::env::InMemoryEnv;

    fn valid_public_key_hex() -> String {
        let sk = SigningKey::from_bytes(&[1u8; 32]);
        hex::encode(sk.verifying_key().as_bytes())
    }

    fn env_with_key() -> InMemoryEnv {
        let env = InMemoryEnv::new();
        env.set("DISCORD_PUBLIC_KEY", &valid_public_key_hex());
        env
    }

    #[test]
    fn defaults_with_required_key() {
        let env = env_with_key();
        let config = DiscordConfig::from_env(&env);

        assert_eq!(config.port, 8080);
        assert_eq!(config.subject_prefix, "discord");
        assert_eq!(config.stream_name, "DISCORD");
        assert_eq!(config.stream_max_age, Duration::from_secs(7 * 24 * 60 * 60));
        assert_eq!(config.nats_ack_timeout, Duration::from_secs(10));
        assert_eq!(config.max_body_size, ByteSize::mib(4));
    }

    #[test]
    fn reads_all_env_vars() {
        let env = InMemoryEnv::new();
        env.set("DISCORD_PUBLIC_KEY", &valid_public_key_hex());
        env.set("DISCORD_WEBHOOK_PORT", "9090");
        env.set("DISCORD_SUBJECT_PREFIX", "dc");
        env.set("DISCORD_STREAM_NAME", "DC_EVENTS");
        env.set("DISCORD_STREAM_MAX_AGE_SECS", "3600");
        env.set("DISCORD_NATS_ACK_TIMEOUT_SECS", "30");
        env.set("DISCORD_MAX_BODY_SIZE", "1048576");

        let config = DiscordConfig::from_env(&env);

        assert_eq!(config.port, 9090);
        assert_eq!(config.subject_prefix, "dc");
        assert_eq!(config.stream_name, "DC_EVENTS");
        assert_eq!(config.stream_max_age, Duration::from_secs(3600));
        assert_eq!(config.nats_ack_timeout, Duration::from_secs(30));
        assert_eq!(config.max_body_size, ByteSize::mib(1));
    }

    #[test]
    #[should_panic(expected = "DISCORD_PUBLIC_KEY is required")]
    fn missing_public_key_panics() {
        let env = InMemoryEnv::new();
        DiscordConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "DISCORD_PUBLIC_KEY is required")]
    fn empty_public_key_panics() {
        let env = InMemoryEnv::new();
        env.set("DISCORD_PUBLIC_KEY", "");
        DiscordConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "DISCORD_PUBLIC_KEY must be a valid")]
    fn invalid_public_key_panics() {
        let env = InMemoryEnv::new();
        env.set("DISCORD_PUBLIC_KEY", "deadbeef");
        DiscordConfig::from_env(&env);
    }

    #[test]
    fn invalid_port_falls_back_to_default() {
        let env = env_with_key();
        env.set("DISCORD_WEBHOOK_PORT", "not-a-number");

        let config = DiscordConfig::from_env(&env);
        assert_eq!(config.port, 8080);
    }

    #[test]
    fn invalid_max_age_falls_back_to_default() {
        let env = env_with_key();
        env.set("DISCORD_STREAM_MAX_AGE_SECS", "not-a-number");

        let config = DiscordConfig::from_env(&env);
        assert_eq!(config.stream_max_age, DEFAULT_STREAM_MAX_AGE);
    }

    #[test]
    fn invalid_nats_ack_timeout_falls_back_to_default() {
        let env = env_with_key();
        env.set("DISCORD_NATS_ACK_TIMEOUT_SECS", "not-a-number");

        let config = DiscordConfig::from_env(&env);
        assert_eq!(config.nats_ack_timeout, DEFAULT_NATS_ACK_TIMEOUT);
    }

    #[test]
    fn invalid_max_body_size_falls_back_to_default() {
        let env = env_with_key();
        env.set("DISCORD_MAX_BODY_SIZE", "not-a-number");

        let config = DiscordConfig::from_env(&env);
        assert_eq!(config.max_body_size, DEFAULT_MAX_BODY_SIZE);
    }
}
