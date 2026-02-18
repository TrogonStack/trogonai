//! Commands sent from NATS agents to the Discord bot

use serde::{Deserialize, Serialize};

use crate::types::{ChannelType, Embed, ModalInput};

/// Send a new message to a channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SendMessageCommand {
    pub channel_id: u64,
    pub content: String,
    #[serde(default)]
    pub embeds: Vec<Embed>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<u64>,
}

/// Edit an existing message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EditMessageCommand {
    pub channel_id: u64,
    pub message_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default)]
    pub embeds: Vec<Embed>,
}

/// Delete a message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteMessageCommand {
    pub channel_id: u64,
    pub message_id: u64,
}

/// Respond to an interaction (must be done within 3 seconds)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InteractionRespondCommand {
    pub interaction_id: u64,
    pub interaction_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default)]
    pub embeds: Vec<Embed>,
    pub ephemeral: bool,
}

/// Defer an interaction response (acknowledge immediately, follow up later)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InteractionDeferCommand {
    pub interaction_id: u64,
    pub interaction_token: String,
    pub ephemeral: bool,
}

/// Send a followup message to a deferred interaction.
///
/// If `session_id` is provided, the bot will register the resulting Discord
/// message in its streaming-state map so that subsequent `StreamMessageCommand`s
/// with the same `session_id` can progressively edit it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InteractionFollowupCommand {
    pub interaction_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default)]
    pub embeds: Vec<Embed>,
    pub ephemeral: bool,
    /// When set, the bot registers the created message under this session_id
    /// so it can be progressively edited via StreamMessageCommand.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// Add a reaction to a message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AddReactionCommand {
    pub channel_id: u64,
    pub message_id: u64,
    /// Unicode emoji or custom emoji in format `name:id`
    pub emoji: String,
}

/// Remove a reaction from a message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RemoveReactionCommand {
    pub channel_id: u64,
    pub message_id: u64,
    /// Unicode emoji or custom emoji in format `name:id`
    pub emoji: String,
}

/// Show the typing indicator in a channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TypingCommand {
    pub channel_id: u64,
}

/// Stream a message progressively (for LLM streaming responses).
///
/// The bot tracks `session_id` → Discord message ID internally.
/// - First command with a given `session_id`: sends a new message (using
///   `reply_to_message_id` if set), then tracks the resulting message ID.
/// - Subsequent commands: edits the tracked message with the updated content.
/// - When `is_final` is true: performs the last edit and cleans up state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamMessageCommand {
    /// Target channel
    pub channel_id: u64,
    /// Accumulated content so far (full text, not just the delta)
    pub content: String,
    /// True when this is the last chunk and no more edits are coming
    pub is_final: bool,
    /// Session identifier used to correlate chunks with a tracked message
    pub session_id: String,
    /// Message to reply to on the *first* send (ignored on edits)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<u64>,
}

/// An autocomplete choice returned to Discord
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutocompleteChoice {
    pub name: String,
    pub value: String,
}

/// Respond to a modal interaction by opening a modal form
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModalRespondCommand {
    pub interaction_id: u64,
    pub interaction_token: String,
    pub custom_id: String,
    pub title: String,
    #[serde(default)]
    pub inputs: Vec<ModalInput>,
}

/// Respond to an autocomplete interaction with choices
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutocompleteRespondCommand {
    pub interaction_id: u64,
    pub interaction_token: String,
    #[serde(default)]
    pub choices: Vec<AutocompleteChoice>,
}

/// Ban a user from a guild
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BanUserCommand {
    pub guild_id: u64,
    pub user_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// How many seconds of messages to delete (0 = none, max 604800 = 7 days)
    pub delete_message_seconds: u64,
}

/// Kick a user from a guild
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KickUserCommand {
    pub guild_id: u64,
    pub user_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Time out a user (0 duration_secs removes the timeout)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TimeoutUserCommand {
    pub guild_id: u64,
    pub user_id: u64,
    /// Duration in seconds. 0 removes the timeout.
    pub duration_secs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Create a new channel in a guild
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateChannelCommand {
    pub guild_id: u64,
    pub name: String,
    pub channel_type: ChannelType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
}

/// Edit an existing channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EditChannelCommand {
    pub channel_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
}

/// Delete a channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteChannelCommand {
    pub channel_id: u64,
}

/// Create a new role in a guild
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateRoleCommand {
    pub guild_id: u64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    pub hoist: bool,
    pub mentionable: bool,
}

/// Assign a role to a guild member
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssignRoleCommand {
    pub guild_id: u64,
    pub user_id: u64,
    pub role_id: u64,
}

/// Remove a role from a guild member
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RemoveRoleCommand {
    pub guild_id: u64,
    pub user_id: u64,
    pub role_id: u64,
}

/// Delete a role from a guild
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteRoleCommand {
    pub guild_id: u64,
    pub role_id: u64,
}

/// Pin a message in a channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PinMessageCommand {
    pub channel_id: u64,
    pub message_id: u64,
}

/// Unpin a message from a channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnpinMessageCommand {
    pub channel_id: u64,
    pub message_id: u64,
}

/// Bulk delete messages from a channel (max 100, must be < 14 days old)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BulkDeleteMessagesCommand {
    pub channel_id: u64,
    pub message_ids: Vec<u64>,
}

/// Create a thread in a channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateThreadCommand {
    pub channel_id: u64,
    pub name: String,
    /// If set, creates the thread from an existing message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<u64>,
    /// Auto-archive duration in minutes (60, 1440, 4320, or 10080)
    pub auto_archive_mins: u16,
}

/// Archive (and optionally lock) a thread
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchiveThreadCommand {
    pub channel_id: u64,
    pub locked: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_send_message_command_roundtrip() {
        roundtrip(&SendMessageCommand {
            channel_id: 100,
            content: "Hello".to_string(),
            embeds: vec![],
            reply_to_message_id: Some(50),
        });
    }

    #[test]
    fn test_send_message_no_reply() {
        roundtrip(&SendMessageCommand {
            channel_id: 100,
            content: "Hello".to_string(),
            embeds: vec![],
            reply_to_message_id: None,
        });
    }

    #[test]
    fn test_edit_message_command_roundtrip() {
        roundtrip(&EditMessageCommand {
            channel_id: 100,
            message_id: 1,
            content: Some("Updated".to_string()),
            embeds: vec![],
        });
    }

    #[test]
    fn test_delete_message_command_roundtrip() {
        roundtrip(&DeleteMessageCommand {
            channel_id: 100,
            message_id: 1,
        });
    }

    #[test]
    fn test_interaction_respond_command_roundtrip() {
        roundtrip(&InteractionRespondCommand {
            interaction_id: 999,
            interaction_token: "tok".to_string(),
            content: Some("pong".to_string()),
            embeds: vec![],
            ephemeral: false,
        });
    }

    #[test]
    fn test_interaction_defer_command_roundtrip() {
        roundtrip(&InteractionDeferCommand {
            interaction_id: 999,
            interaction_token: "tok".to_string(),
            ephemeral: true,
        });
    }

    #[test]
    fn test_interaction_followup_command_roundtrip() {
        roundtrip(&InteractionFollowupCommand {
            interaction_token: "tok".to_string(),
            content: Some("followup".to_string()),
            embeds: vec![],
            ephemeral: false,
            session_id: None,
        });
    }

    #[test]
    fn test_interaction_followup_command_with_session() {
        roundtrip(&InteractionFollowupCommand {
            interaction_token: "tok".to_string(),
            content: Some("▌".to_string()),
            embeds: vec![],
            ephemeral: false,
            session_id: Some("ask_guild_100_200_300".to_string()),
        });
        // session_id must be omitted when None
        let cmd = InteractionFollowupCommand {
            interaction_token: "tok".to_string(),
            content: None,
            embeds: vec![],
            ephemeral: false,
            session_id: None,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(
            !json.contains("session_id"),
            "session_id must be omitted when None"
        );
    }

    #[test]
    fn test_add_reaction_command_roundtrip() {
        roundtrip(&AddReactionCommand {
            channel_id: 100,
            message_id: 1,
            emoji: "👍".to_string(),
        });
    }

    #[test]
    fn test_remove_reaction_command_roundtrip() {
        roundtrip(&RemoveReactionCommand {
            channel_id: 100,
            message_id: 1,
            emoji: "wave:123456".to_string(),
        });
    }

    #[test]
    fn test_typing_command_roundtrip() {
        roundtrip(&TypingCommand { channel_id: 100 });
    }

    #[test]
    fn test_stream_message_command_roundtrip() {
        roundtrip(&StreamMessageCommand {
            channel_id: 100,
            content: "Hello ▌".to_string(),
            is_final: false,
            session_id: "guild_100_200".to_string(),
            reply_to_message_id: Some(42),
        });
    }

    #[test]
    fn test_stream_message_command_final() {
        roundtrip(&StreamMessageCommand {
            channel_id: 100,
            content: "Hello, world!".to_string(),
            is_final: true,
            session_id: "guild_100_200".to_string(),
            reply_to_message_id: None,
        });
    }

    #[test]
    fn test_stream_message_command_no_reply() {
        let cmd = StreamMessageCommand {
            channel_id: 1,
            content: "chunk".to_string(),
            is_final: false,
            session_id: "s1".to_string(),
            reply_to_message_id: None,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(
            !json.contains("reply_to_message_id"),
            "optional field must be omitted"
        );
    }

    #[test]
    fn test_autocomplete_choice_roundtrip() {
        roundtrip(&AutocompleteChoice {
            name: "Option A".to_string(),
            value: "a".to_string(),
        });
    }

    #[test]
    fn test_modal_respond_command_roundtrip() {
        roundtrip(&ModalRespondCommand {
            interaction_id: 1234,
            interaction_token: "tok".to_string(),
            custom_id: "my_modal".to_string(),
            title: "Fill in details".to_string(),
            inputs: vec![ModalInput {
                custom_id: "name".to_string(),
                value: "placeholder".to_string(),
            }],
        });
    }

    #[test]
    fn test_modal_respond_command_empty_inputs() {
        let cmd = ModalRespondCommand {
            interaction_id: 1,
            interaction_token: "tok".to_string(),
            custom_id: "m".to_string(),
            title: "T".to_string(),
            inputs: vec![],
        };
        // inputs uses #[serde(default)] — empty vec must still round-trip
        roundtrip(&cmd);
    }

    #[test]
    fn test_autocomplete_respond_command_roundtrip() {
        roundtrip(&AutocompleteRespondCommand {
            interaction_id: 5555,
            interaction_token: "ac-tok".to_string(),
            choices: vec![
                AutocompleteChoice {
                    name: "foo".to_string(),
                    value: "foo".to_string(),
                },
                AutocompleteChoice {
                    name: "bar".to_string(),
                    value: "bar".to_string(),
                },
            ],
        });
    }

    #[test]
    fn test_autocomplete_respond_command_empty_choices() {
        roundtrip(&AutocompleteRespondCommand {
            interaction_id: 1,
            interaction_token: "tok".to_string(),
            choices: vec![],
        });
    }

    #[test]
    fn test_ban_user_command_roundtrip() {
        roundtrip(&BanUserCommand {
            guild_id: 200,
            user_id: 42,
            reason: Some("Spamming".to_string()),
            delete_message_seconds: 86400,
        });
    }

    #[test]
    fn test_ban_user_command_no_reason_omitted() {
        let cmd = BanUserCommand {
            guild_id: 200,
            user_id: 42,
            reason: None,
            delete_message_seconds: 0,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(!json.contains("reason"), "reason must be omitted when None");
        roundtrip(&cmd);
    }

    #[test]
    fn test_kick_user_command_roundtrip() {
        roundtrip(&KickUserCommand {
            guild_id: 200,
            user_id: 42,
            reason: Some("Rule violation".to_string()),
        });
    }

    #[test]
    fn test_timeout_user_command_roundtrip() {
        roundtrip(&TimeoutUserCommand {
            guild_id: 200,
            user_id: 42,
            duration_secs: 3600,
            reason: Some("Cooling off".to_string()),
        });
    }

    #[test]
    fn test_timeout_user_remove_timeout() {
        // duration_secs = 0 means remove timeout
        roundtrip(&TimeoutUserCommand {
            guild_id: 200,
            user_id: 42,
            duration_secs: 0,
            reason: None,
        });
    }

    #[test]
    fn test_create_channel_command_roundtrip() {
        use crate::types::ChannelType;
        roundtrip(&CreateChannelCommand {
            guild_id: 200,
            name: "announcements".to_string(),
            channel_type: ChannelType::GuildText,
            category_id: Some(999),
            topic: Some("Official news".to_string()),
        });
    }

    #[test]
    fn test_create_channel_command_optional_omitted() {
        use crate::types::ChannelType;
        let cmd = CreateChannelCommand {
            guild_id: 200,
            name: "voice-chat".to_string(),
            channel_type: ChannelType::GuildVoice,
            category_id: None,
            topic: None,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(!json.contains("category_id"), "category_id must be omitted when None");
        assert!(!json.contains("topic"), "topic must be omitted when None");
        roundtrip(&cmd);
    }

    #[test]
    fn test_edit_channel_command_roundtrip() {
        roundtrip(&EditChannelCommand {
            channel_id: 300,
            name: Some("renamed".to_string()),
            topic: Some("New topic".to_string()),
        });
    }

    #[test]
    fn test_delete_channel_command_roundtrip() {
        roundtrip(&DeleteChannelCommand { channel_id: 300 });
    }

    #[test]
    fn test_create_role_command_roundtrip() {
        roundtrip(&CreateRoleCommand {
            guild_id: 200,
            name: "VIP".to_string(),
            color: Some(0xFFD700),
            hoist: true,
            mentionable: false,
        });
    }

    #[test]
    fn test_create_role_command_no_color_omitted() {
        let cmd = CreateRoleCommand {
            guild_id: 200,
            name: "member".to_string(),
            color: None,
            hoist: false,
            mentionable: false,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(!json.contains("color"), "color must be omitted when None");
        roundtrip(&cmd);
    }

    #[test]
    fn test_assign_role_command_roundtrip() {
        roundtrip(&AssignRoleCommand {
            guild_id: 200,
            user_id: 42,
            role_id: 111,
        });
    }

    #[test]
    fn test_remove_role_command_roundtrip() {
        roundtrip(&RemoveRoleCommand {
            guild_id: 200,
            user_id: 42,
            role_id: 111,
        });
    }

    #[test]
    fn test_delete_role_command_roundtrip() {
        roundtrip(&DeleteRoleCommand {
            guild_id: 200,
            role_id: 111,
        });
    }

    #[test]
    fn test_pin_message_command_roundtrip() {
        roundtrip(&PinMessageCommand {
            channel_id: 100,
            message_id: 50,
        });
    }

    #[test]
    fn test_unpin_message_command_roundtrip() {
        roundtrip(&UnpinMessageCommand {
            channel_id: 100,
            message_id: 50,
        });
    }

    #[test]
    fn test_bulk_delete_messages_command_roundtrip() {
        roundtrip(&BulkDeleteMessagesCommand {
            channel_id: 100,
            message_ids: vec![1, 2, 3, 4, 5],
        });
    }

    #[test]
    fn test_bulk_delete_messages_command_empty() {
        roundtrip(&BulkDeleteMessagesCommand {
            channel_id: 100,
            message_ids: vec![],
        });
    }

    #[test]
    fn test_create_thread_command_roundtrip() {
        roundtrip(&CreateThreadCommand {
            channel_id: 100,
            name: "Discussion thread".to_string(),
            message_id: Some(50),
            auto_archive_mins: 1440,
        });
    }

    #[test]
    fn test_create_thread_command_standalone() {
        // No message_id = standalone thread
        let cmd = CreateThreadCommand {
            channel_id: 100,
            name: "New thread".to_string(),
            message_id: None,
            auto_archive_mins: 60,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(!json.contains("message_id"), "message_id must be omitted when None");
        roundtrip(&cmd);
    }

    #[test]
    fn test_archive_thread_command_roundtrip() {
        roundtrip(&ArchiveThreadCommand {
            channel_id: 100,
            locked: true,
        });
    }
}
