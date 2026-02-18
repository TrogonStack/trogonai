//! Commands sent from NATS agents to the Discord bot

use serde::{Deserialize, Serialize};

use crate::types::Embed;

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
}
