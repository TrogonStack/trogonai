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

/// Send a followup message to a deferred interaction
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InteractionFollowupCommand {
    pub interaction_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default)]
    pub embeds: Vec<Embed>,
    pub ephemeral: bool,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T: serde::Serialize + for<'de> serde::Deserialize<'de> + PartialEq + std::fmt::Debug>(
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
        });
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
}
