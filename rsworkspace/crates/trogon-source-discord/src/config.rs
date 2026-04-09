use std::fmt;

use ed25519_dalek::VerifyingKey;
use trogon_nats::NatsToken;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_std::{EmptySecret, NonZeroDuration, SecretString};

#[derive(Clone)]
pub struct DiscordBotToken(SecretString);

impl DiscordBotToken {
    pub fn new(s: impl AsRef<str>) -> Result<Self, EmptySecret> {
        SecretString::new(s).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for DiscordBotToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DiscordBotToken(****)")
    }
}
use twilight_model::gateway::Intents;

#[derive(Clone)]
pub enum SourceMode {
    Gateway {
        bot_token: DiscordBotToken,
        intents: Intents,
    },
    Webhook {
        public_key: VerifyingKey,
    },
}

#[derive(Clone)]
pub struct DiscordConfig {
    pub mode: SourceMode,
    pub subject_prefix: NatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub nats_ack_timeout: NonZeroDuration,
    pub nats_request_timeout: NonZeroDuration,
}

const PRIVILEGED_INTENTS: Intents = Intents::from_bits_truncate(
    Intents::GUILD_MEMBERS.bits()
        | Intents::GUILD_PRESENCES.bits()
        | Intents::MESSAGE_CONTENT.bits(),
);

pub fn default_intents() -> Intents {
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

pub fn parse_gateway_intents(s: &str) -> Result<Intents, UnknownIntentError> {
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
    fn parse_intents_is_case_insensitive_and_trimmed() {
        let intents =
            parse_gateway_intents("  GUILDS , Guild_Members , direct_message_reactions ").unwrap();
        assert!(intents.contains(Intents::GUILDS));
        assert!(intents.contains(Intents::GUILD_MEMBERS));
        assert!(intents.contains(Intents::DIRECT_MESSAGE_REACTIONS));
    }

    #[test]
    fn parse_intents_accepts_all_supported_keywords() {
        let intents = parse_gateway_intents(
            "guilds,guild_members,guild_moderation,guild_emojis_and_stickers,\
             guild_integrations,guild_webhooks,guild_invites,guild_voice_states,\
             guild_presences,guild_messages,guild_message_reactions,guild_message_typing,\
             direct_messages,direct_message_reactions,direct_message_typing,message_content,\
             guild_scheduled_events,auto_moderation_configuration,auto_moderation_execution,\
             guild_message_polls,direct_message_polls,all,non_privileged",
        )
        .unwrap();

        assert_eq!(intents, Intents::all());
    }

    #[test]
    fn discord_bot_token_roundtrips() {
        let token = DiscordBotToken::new("Bot token").unwrap();
        assert_eq!(token.as_str(), "Bot token");
    }

    #[test]
    fn discord_bot_token_debug_redacts() {
        let token = DiscordBotToken::new("Bot token").unwrap();
        assert_eq!(format!("{token:?}"), "DiscordBotToken(****)");
    }
}
