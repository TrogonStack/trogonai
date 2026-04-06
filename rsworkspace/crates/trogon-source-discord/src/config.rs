use std::time::Duration;

use ed25519_dalek::VerifyingKey;
use trogon_nats::{NatsConfig, NatsToken};
use trogon_std::env::ReadEnv;
use twilight_model::gateway::Intents;

use std::fmt;

use crate::constants::{
    DEFAULT_NATS_ACK_TIMEOUT, DEFAULT_NATS_REQUEST_TIMEOUT, DEFAULT_PORT, DEFAULT_STREAM_MAX_AGE,
    DEFAULT_STREAM_NAME, DEFAULT_SUBJECT_PREFIX,
};
use crate::signature;

pub enum SourceMode {
    Gateway { bot_token: String, intents: Intents },
    Webhook { public_key: VerifyingKey },
}

pub struct DiscordConfig {
    pub mode: SourceMode,
    pub port: u16,
    pub subject_prefix: NatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: Duration,
    pub nats_ack_timeout: Duration,
    pub nats_request_timeout: Duration,
    pub nats: NatsConfig,
}

impl DiscordConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let mode_str = env
            .var("DISCORD_MODE")
            .expect("DISCORD_MODE is required (gateway or webhook)");

        let mode = match mode_str.to_ascii_lowercase().as_str() {
            "gateway" => {
                let bot_token = env
                    .var("DISCORD_BOT_TOKEN")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .expect("DISCORD_BOT_TOKEN is required when DISCORD_MODE=gateway");

                let intents = env
                    .var("DISCORD_GATEWAY_INTENTS")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .map(|s| parse_gateway_intents(&s).expect("DISCORD_GATEWAY_INTENTS is invalid"))
                    .unwrap_or_else(default_intents);

                SourceMode::Gateway { bot_token, intents }
            }
            "webhook" => {
                let public_key_hex = env
                    .var("DISCORD_PUBLIC_KEY")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .expect("DISCORD_PUBLIC_KEY is required when DISCORD_MODE=webhook");

                let public_key = signature::parse_public_key(&public_key_hex)
                    .expect("DISCORD_PUBLIC_KEY must be a valid hex-encoded Ed25519 public key");

                SourceMode::Webhook { public_key }
            }
            other => panic!("DISCORD_MODE must be 'gateway' or 'webhook', got '{other}'"),
        };

        Self {
            mode,
            port: env
                .var("DISCORD_WEBHOOK_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            subject_prefix: NatsToken::new(
                env.var("DISCORD_SUBJECT_PREFIX")
                    .unwrap_or_else(|_| DEFAULT_SUBJECT_PREFIX.to_string()),
            )
            .expect("DISCORD_SUBJECT_PREFIX is not a valid NATS token"),
            stream_name: NatsToken::new(
                env.var("DISCORD_STREAM_NAME")
                    .unwrap_or_else(|_| DEFAULT_STREAM_NAME.to_string()),
            )
            .expect("DISCORD_STREAM_NAME is not a valid NATS token"),
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
            nats_request_timeout: env
                .var("DISCORD_NATS_REQUEST_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_NATS_REQUEST_TIMEOUT),
            nats: NatsConfig::from_env(env),
        }
    }
}

const PRIVILEGED_INTENTS: Intents = Intents::from_bits_truncate(
    Intents::GUILD_MEMBERS.bits()
        | Intents::GUILD_PRESENCES.bits()
        | Intents::MESSAGE_CONTENT.bits(),
);

fn default_intents() -> Intents {
    Intents::all().difference(PRIVILEGED_INTENTS)
}

#[derive(Debug)]
pub struct UnknownIntentError {
    intent: String,
}

impl fmt::Display for UnknownIntentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown gateway intent: '{}'", self.intent)
    }
}

impl std::error::Error for UnknownIntentError {}

fn parse_gateway_intents(s: &str) -> Result<Intents, UnknownIntentError> {
    let mut intents = Intents::empty();
    for part in s.split(',') {
        let part = part.trim();
        intents |= match part.to_ascii_lowercase().as_str() {
            "guilds" => Intents::GUILDS,
            "guild_members" => Intents::GUILD_MEMBERS,
            "guild_moderation" => Intents::GUILD_MODERATION,
            "guild_emojis_and_stickers" => Intents::GUILD_EMOJIS_AND_STICKERS,
            "guild_integrations" => Intents::GUILD_INTEGRATIONS,
            "guild_webhooks" => Intents::GUILD_WEBHOOKS,
            "guild_invites" => Intents::GUILD_INVITES,
            "guild_voice_states" => Intents::GUILD_VOICE_STATES,
            "guild_presences" => Intents::GUILD_PRESENCES,
            "guild_messages" => Intents::GUILD_MESSAGES,
            "guild_message_reactions" => Intents::GUILD_MESSAGE_REACTIONS,
            "guild_message_typing" => Intents::GUILD_MESSAGE_TYPING,
            "direct_messages" => Intents::DIRECT_MESSAGES,
            "direct_message_reactions" => Intents::DIRECT_MESSAGE_REACTIONS,
            "direct_message_typing" => Intents::DIRECT_MESSAGE_TYPING,
            "message_content" => Intents::MESSAGE_CONTENT,
            "guild_scheduled_events" => Intents::GUILD_SCHEDULED_EVENTS,
            "auto_moderation_configuration" => Intents::AUTO_MODERATION_CONFIGURATION,
            "auto_moderation_execution" => Intents::AUTO_MODERATION_EXECUTION,
            "guild_message_polls" => Intents::GUILD_MESSAGE_POLLS,
            "direct_message_polls" => Intents::DIRECT_MESSAGE_POLLS,
            "all" => Intents::all(),
            "non_privileged" => default_intents(),
            _ => {
                return Err(UnknownIntentError {
                    intent: part.to_string(),
                });
            }
        };
    }
    Ok(intents)
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

    fn env_webhook() -> InMemoryEnv {
        let env = InMemoryEnv::new();
        env.set("DISCORD_MODE", "webhook");
        env.set("DISCORD_PUBLIC_KEY", valid_public_key_hex());
        env
    }

    fn env_gateway() -> InMemoryEnv {
        let env = InMemoryEnv::new();
        env.set("DISCORD_MODE", "gateway");
        env.set("DISCORD_BOT_TOKEN", "test-token");
        env
    }

    #[test]
    fn webhook_mode_defaults() {
        let env = env_webhook();
        let config = DiscordConfig::from_env(&env);

        assert!(matches!(config.mode, SourceMode::Webhook { .. }));
        assert_eq!(config.port, 8080);
        assert_eq!(config.subject_prefix.as_str(), "discord");
        assert_eq!(config.stream_name.as_str(), "DISCORD");
        assert_eq!(config.stream_max_age, Duration::from_secs(7 * 24 * 60 * 60));
        assert_eq!(config.nats_ack_timeout, Duration::from_secs(10));
        assert_eq!(config.nats_request_timeout, Duration::from_secs(2));
    }

    #[test]
    fn gateway_mode_defaults() {
        let env = env_gateway();
        let config = DiscordConfig::from_env(&env);

        match &config.mode {
            SourceMode::Gateway { bot_token, intents } => {
                assert_eq!(bot_token, "test-token");
                assert_eq!(*intents, default_intents());
                assert!(intents.contains(Intents::GUILDS));
                assert!(intents.contains(Intents::GUILD_MESSAGES));
                assert!(!intents.contains(Intents::MESSAGE_CONTENT));
                assert!(!intents.contains(Intents::GUILD_MEMBERS));
                assert!(!intents.contains(Intents::GUILD_PRESENCES));
            }
            SourceMode::Webhook { .. } => panic!("expected gateway mode"),
        }
    }

    #[test]
    #[should_panic(expected = "DISCORD_MODE is required")]
    fn missing_mode_panics() {
        let env = InMemoryEnv::new();
        DiscordConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "DISCORD_MODE must be 'gateway' or 'webhook'")]
    fn invalid_mode_panics() {
        let env = InMemoryEnv::new();
        env.set("DISCORD_MODE", "bogus");
        DiscordConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "DISCORD_BOT_TOKEN is required when DISCORD_MODE=gateway")]
    fn gateway_missing_token_panics() {
        let env = InMemoryEnv::new();
        env.set("DISCORD_MODE", "gateway");
        DiscordConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "DISCORD_PUBLIC_KEY is required when DISCORD_MODE=webhook")]
    fn webhook_missing_key_panics() {
        let env = InMemoryEnv::new();
        env.set("DISCORD_MODE", "webhook");
        DiscordConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "DISCORD_PUBLIC_KEY must be a valid")]
    fn webhook_invalid_key_panics() {
        let env = InMemoryEnv::new();
        env.set("DISCORD_MODE", "webhook");
        env.set("DISCORD_PUBLIC_KEY", "deadbeef");
        DiscordConfig::from_env(&env);
    }

    #[test]
    fn reads_all_env_vars() {
        let env = env_webhook();
        env.set("DISCORD_WEBHOOK_PORT", "9090");
        env.set("DISCORD_SUBJECT_PREFIX", "dc");
        env.set("DISCORD_STREAM_NAME", "DC_EVENTS");
        env.set("DISCORD_STREAM_MAX_AGE_SECS", "3600");
        env.set("DISCORD_NATS_ACK_TIMEOUT_SECS", "30");
        env.set("DISCORD_NATS_REQUEST_TIMEOUT_SECS", "5");

        let config = DiscordConfig::from_env(&env);

        assert_eq!(config.port, 9090);
        assert_eq!(config.subject_prefix.as_str(), "dc");
        assert_eq!(config.stream_name.as_str(), "DC_EVENTS");
        assert_eq!(config.stream_max_age, Duration::from_secs(3600));
        assert_eq!(config.nats_ack_timeout, Duration::from_secs(30));
        assert_eq!(config.nats_request_timeout, Duration::from_secs(5));
    }

    #[test]
    fn invalid_port_falls_back_to_default() {
        let env = env_webhook();
        env.set("DISCORD_WEBHOOK_PORT", "not-a-number");

        let config = DiscordConfig::from_env(&env);
        assert_eq!(config.port, 8080);
    }

    #[test]
    fn invalid_max_age_falls_back_to_default() {
        let env = env_webhook();
        env.set("DISCORD_STREAM_MAX_AGE_SECS", "not-a-number");

        let config = DiscordConfig::from_env(&env);
        assert_eq!(config.stream_max_age, DEFAULT_STREAM_MAX_AGE);
    }

    #[test]
    fn invalid_nats_ack_timeout_falls_back_to_default() {
        let env = env_webhook();
        env.set("DISCORD_NATS_ACK_TIMEOUT_SECS", "not-a-number");

        let config = DiscordConfig::from_env(&env);
        assert_eq!(config.nats_ack_timeout, DEFAULT_NATS_ACK_TIMEOUT);
    }

    #[test]
    fn invalid_nats_request_timeout_falls_back_to_default() {
        let env = env_webhook();
        env.set("DISCORD_NATS_REQUEST_TIMEOUT_SECS", "not-a-number");

        let config = DiscordConfig::from_env(&env);
        assert_eq!(config.nats_request_timeout, DEFAULT_NATS_REQUEST_TIMEOUT);
    }

    #[test]
    fn parse_intents_csv() {
        let intents = parse_gateway_intents("guilds,guild_messages,message_content").unwrap();
        assert!(intents.contains(Intents::GUILDS));
        assert!(intents.contains(Intents::GUILD_MESSAGES));
        assert!(intents.contains(Intents::MESSAGE_CONTENT));
        assert!(!intents.contains(Intents::GUILD_PRESENCES));
    }

    #[test]
    fn parse_intents_all() {
        let intents = parse_gateway_intents("all").unwrap();
        assert_eq!(intents, Intents::all());
    }

    #[test]
    fn parse_intents_non_privileged() {
        let intents = parse_gateway_intents("non_privileged").unwrap();
        assert_eq!(intents, default_intents());
    }

    #[test]
    fn parse_intents_unknown_returns_error() {
        let err = parse_gateway_intents("guilds,bogus_intent,guild_messages").unwrap_err();
        assert_eq!(err.intent, "bogus_intent");
        assert_eq!(err.to_string(), "unknown gateway intent: 'bogus_intent'");
        assert!(std::error::Error::source(&err).is_none());
    }

    #[test]
    #[should_panic(expected = "DISCORD_GATEWAY_INTENTS is invalid")]
    fn gateway_invalid_intents_panics() {
        let env = env_gateway();
        env.set("DISCORD_GATEWAY_INTENTS", "guilds,not_real");
        DiscordConfig::from_env(&env);
    }

    #[test]
    fn gateway_intents_from_env() {
        let env = env_gateway();
        env.set("DISCORD_GATEWAY_INTENTS", "guilds,guild_presences");
        let config = DiscordConfig::from_env(&env);
        match &config.mode {
            SourceMode::Gateway { intents, .. } => {
                assert!(intents.contains(Intents::GUILDS));
                assert!(intents.contains(Intents::GUILD_PRESENCES));
                assert!(!intents.contains(Intents::MESSAGE_CONTENT));
            }
            _ => panic!("expected gateway mode"),
        }
    }

    #[test]
    fn mode_is_case_insensitive() {
        let env = InMemoryEnv::new();
        env.set("DISCORD_MODE", "Gateway");
        env.set("DISCORD_BOT_TOKEN", "tok");
        let config = DiscordConfig::from_env(&env);
        assert!(matches!(config.mode, SourceMode::Gateway { .. }));

        let env = InMemoryEnv::new();
        env.set("DISCORD_MODE", "WEBHOOK");
        env.set("DISCORD_PUBLIC_KEY", valid_public_key_hex());
        let config = DiscordConfig::from_env(&env);
        assert!(matches!(config.mode, SourceMode::Webhook { .. }));
    }

    #[test]
    fn default_intents_excludes_privileged() {
        let intents = default_intents();
        assert!(!intents.contains(Intents::GUILD_MEMBERS));
        assert!(!intents.contains(Intents::GUILD_PRESENCES));
        assert!(!intents.contains(Intents::MESSAGE_CONTENT));
        assert!(intents.contains(Intents::GUILDS));
        assert!(intents.contains(Intents::GUILD_MESSAGES));
        assert!(intents.contains(Intents::GUILD_SCHEDULED_EVENTS));
    }

    #[test]
    fn parse_intents_includes_poll_intents() {
        let intents = parse_gateway_intents("guild_message_polls,direct_message_polls").unwrap();
        assert!(intents.contains(Intents::GUILD_MESSAGE_POLLS));
        assert!(intents.contains(Intents::DIRECT_MESSAGE_POLLS));
    }

    #[test]
    #[should_panic(expected = "DISCORD_SUBJECT_PREFIX is not a valid NATS token")]
    fn invalid_subject_prefix_panics() {
        let env = env_webhook();
        env.set("DISCORD_SUBJECT_PREFIX", "has spaces");
        DiscordConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "DISCORD_STREAM_NAME is not a valid NATS token")]
    fn invalid_stream_name_panics() {
        let env = env_webhook();
        env.set("DISCORD_STREAM_NAME", "has.dots");
        DiscordConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "DISCORD_SUBJECT_PREFIX is not a valid NATS token")]
    fn empty_subject_prefix_panics() {
        let env = env_webhook();
        env.set("DISCORD_SUBJECT_PREFIX", "");
        DiscordConfig::from_env(&env);
    }

    #[test]
    #[should_panic(expected = "DISCORD_STREAM_NAME is not a valid NATS token")]
    fn wildcard_stream_name_panics() {
        let env = env_webhook();
        env.set("DISCORD_STREAM_NAME", "DISCORD>");
        DiscordConfig::from_env(&env);
    }
}
