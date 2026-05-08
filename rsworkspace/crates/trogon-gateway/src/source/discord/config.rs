use std::fmt;

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
pub struct DiscordConfig {
    pub bot_token: DiscordBotToken,
    pub intents: Intents,
    pub subject_prefix: NatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub nats_ack_timeout: NonZeroDuration,
}

const PRIVILEGED_INTENTS: Intents = Intents::from_bits_truncate(
    Intents::GUILD_MEMBERS.bits() | Intents::GUILD_PRESENCES.bits() | Intents::MESSAGE_CONTENT.bits(),
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
mod tests;
