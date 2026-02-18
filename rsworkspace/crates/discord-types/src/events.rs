//! Events published from Discord bot to NATS

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::{
    CommandOption, ComponentType, DiscordMember, DiscordMessage, DiscordUser, Embed, Emoji,
};

/// Base event metadata shared across all Discord events
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventMetadata {
    /// Unique event ID
    pub event_id: Uuid,
    /// Session ID (format: dc-dm-{channel_id} or dc-guild-{guild_id}-{channel_id})
    pub session_id: String,
    /// Event timestamp
    pub timestamp: DateTime<Utc>,
    /// Monotonic sequence number
    pub sequence: u64,
}

impl EventMetadata {
    /// Create new event metadata
    pub fn new(session_id: impl Into<String>, sequence: u64) -> Self {
        Self {
            event_id: Uuid::new_v4(),
            session_id: session_id.into(),
            timestamp: Utc::now(),
            sequence,
        }
    }
}

/// A new message was created in a channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageCreatedEvent {
    pub metadata: EventMetadata,
    pub message: DiscordMessage,
}

/// A message was edited
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageUpdatedEvent {
    pub metadata: EventMetadata,
    pub message_id: u64,
    pub channel_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_content: Option<String>,
    #[serde(default)]
    pub new_embeds: Vec<Embed>,
}

/// A message was deleted
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageDeletedEvent {
    pub metadata: EventMetadata,
    pub message_id: u64,
    pub channel_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
}

/// A slash command was invoked
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SlashCommandEvent {
    pub metadata: EventMetadata,
    pub interaction_id: u64,
    pub interaction_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    pub channel_id: u64,
    pub user: DiscordUser,
    pub command_name: String,
    #[serde(default)]
    pub options: Vec<CommandOption>,
}

/// A message component (button/select) was interacted with
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComponentInteractionEvent {
    pub metadata: EventMetadata,
    pub interaction_id: u64,
    pub interaction_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    pub channel_id: u64,
    pub user: DiscordUser,
    pub message_id: u64,
    pub custom_id: String,
    pub component_type: ComponentType,
}

/// A reaction was added to a message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReactionAddEvent {
    pub metadata: EventMetadata,
    pub user_id: u64,
    pub channel_id: u64,
    pub message_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    pub emoji: Emoji,
}

/// A reaction was removed from a message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReactionRemoveEvent {
    pub metadata: EventMetadata,
    pub user_id: u64,
    pub channel_id: u64,
    pub message_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    pub emoji: Emoji,
}

/// A member joined a guild
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildMemberAddEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub member: DiscordMember,
}

/// A member left or was removed from a guild
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildMemberRemoveEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub user: DiscordUser,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DiscordUser;

    fn test_meta() -> EventMetadata {
        EventMetadata::new("dc-dm-123", 1)
    }

    fn test_user() -> DiscordUser {
        DiscordUser {
            id: 42,
            username: "alice".to_string(),
            global_name: None,
            bot: false,
        }
    }

    fn roundtrip<
        T: serde::Serialize + for<'de> serde::Deserialize<'de> + PartialEq + std::fmt::Debug,
    >(
        val: &T,
    ) {
        let json = serde_json::to_string(val).expect("serialize");
        let back: T = serde_json::from_str(&json).expect("deserialize");
        let json2 = serde_json::to_string(&back).expect("re-serialize");
        assert_eq!(json, json2, "roundtrip produced different JSON");
    }

    #[test]
    fn test_event_metadata_new() {
        let meta = EventMetadata::new("dc-dm-99", 42);
        assert_eq!(meta.session_id, "dc-dm-99");
        assert_eq!(meta.sequence, 42);
        assert_ne!(
            meta.event_id.to_string(),
            "00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn test_event_metadata_roundtrip() {
        roundtrip(&test_meta());
    }

    #[test]
    fn test_message_created_event_roundtrip() {
        let event = MessageCreatedEvent {
            metadata: test_meta(),
            message: crate::types::DiscordMessage {
                id: 1,
                channel_id: 100,
                guild_id: Some(200),
                author: test_user(),
                content: "Hello".to_string(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                edited_timestamp: None,
                attachments: vec![],
                embeds: vec![],
                referenced_message_id: None,
                referenced_message_content: None,
            },
        };
        roundtrip(&event);
    }

    #[test]
    fn test_message_updated_event_roundtrip() {
        let event = MessageUpdatedEvent {
            metadata: test_meta(),
            message_id: 1,
            channel_id: 100,
            guild_id: Some(200),
            new_content: Some("Updated".to_string()),
            new_embeds: vec![],
        };
        roundtrip(&event);
    }

    #[test]
    fn test_message_deleted_event_roundtrip() {
        let event = MessageDeletedEvent {
            metadata: test_meta(),
            message_id: 1,
            channel_id: 100,
            guild_id: None,
        };
        roundtrip(&event);
    }

    #[test]
    fn test_slash_command_event_roundtrip() {
        use crate::types::CommandOptionValue;
        let event = SlashCommandEvent {
            metadata: test_meta(),
            interaction_id: 9999,
            interaction_token: "token123".to_string(),
            guild_id: Some(200),
            channel_id: 100,
            user: test_user(),
            command_name: "ping".to_string(),
            options: vec![crate::types::CommandOption {
                name: "message".to_string(),
                value: CommandOptionValue::String("hello".to_string()),
            }],
        };
        roundtrip(&event);
    }

    #[test]
    fn test_component_interaction_event_roundtrip() {
        let event = ComponentInteractionEvent {
            metadata: test_meta(),
            interaction_id: 8888,
            interaction_token: "tok".to_string(),
            guild_id: None,
            channel_id: 100,
            user: test_user(),
            message_id: 5,
            custom_id: "button:ok".to_string(),
            component_type: ComponentType::Button,
        };
        roundtrip(&event);
    }

    #[test]
    fn test_reaction_add_event_roundtrip() {
        let event = ReactionAddEvent {
            metadata: test_meta(),
            user_id: 42,
            channel_id: 100,
            message_id: 1,
            guild_id: Some(200),
            emoji: Emoji {
                id: None,
                name: "👍".to_string(),
                animated: false,
            },
        };
        roundtrip(&event);
    }

    #[test]
    fn test_reaction_remove_event_roundtrip() {
        let event = ReactionRemoveEvent {
            metadata: test_meta(),
            user_id: 42,
            channel_id: 100,
            message_id: 1,
            guild_id: None,
            emoji: Emoji {
                id: Some(123456),
                name: "wave".to_string(),
                animated: true,
            },
        };
        roundtrip(&event);
    }

    #[test]
    fn test_guild_member_add_event_roundtrip() {
        let event = GuildMemberAddEvent {
            metadata: test_meta(),
            guild_id: 200,
            member: DiscordMember {
                user: test_user(),
                guild_id: 200,
                nick: Some("alice_nick".to_string()),
                roles: vec![111, 222],
            },
        };
        roundtrip(&event);
    }

    #[test]
    fn test_guild_member_remove_event_roundtrip() {
        let event = GuildMemberRemoveEvent {
            metadata: test_meta(),
            guild_id: 200,
            user: test_user(),
        };
        roundtrip(&event);
    }
}
