//! Core Discord domain types

use serde::{Deserialize, Serialize};

/// Discord user
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscordUser {
    pub id: u64,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_name: Option<String>,
    pub bot: bool,
}

/// Discord guild (server)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscordGuild {
    pub id: u64,
    pub name: String,
}

/// Channel type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelType {
    GuildText,
    Dm,
    GuildVoice,
    GroupDm,
    GuildCategory,
    GuildNews,
    GuildStageVoice,
    GuildForum,
    Unknown,
}

/// Discord channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscordChannel {
    pub id: u64,
    pub channel_type: ChannelType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Discord guild member
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscordMember {
    pub user: DiscordUser,
    pub guild_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nick: Option<String>,
    pub roles: Vec<u64>,
}

/// Message attachment
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Attachment {
    pub id: u64,
    pub filename: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub size: u64,
}

/// Embed field
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

/// Message embed
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Embed {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub fields: Vec<EmbedField>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
}

/// Discord emoji
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Emoji {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub name: String,
    pub animated: bool,
}

/// Command option value
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CommandOptionValue {
    String(String),
    Integer(i64),
    Boolean(bool),
    User(u64),
    Channel(u64),
    Role(u64),
    Number(f64),
}

/// Command option (slash command argument)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandOption {
    pub name: String,
    pub value: CommandOptionValue,
}

/// Component type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComponentType {
    ActionRow,
    Button,
    StringSelect,
    TextInput,
    UserSelect,
    RoleSelect,
    MentionableSelect,
    ChannelSelect,
    Unknown,
}

/// Discord message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscordMessage {
    pub id: u64,
    pub channel_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    pub author: DiscordUser,
    pub content: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edited_timestamp: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub embeds: Vec<Embed>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub referenced_message_id: Option<u64>,
    /// Content of the message being replied to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referenced_message_content: Option<String>,
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
        assert_eq!(*val, back);
    }

    #[test]
    fn test_discord_user_roundtrip() {
        let user = DiscordUser {
            id: 123456789,
            username: "testuser".to_string(),
            global_name: Some("Test User".to_string()),
            bot: false,
        };
        roundtrip(&user);
    }

    #[test]
    fn test_channel_type_serde() {
        for (t, expected) in [
            (ChannelType::GuildText, "\"guild_text\""),
            (ChannelType::Dm, "\"dm\""),
            (ChannelType::GuildVoice, "\"guild_voice\""),
        ] {
            let json = serde_json::to_string(&t).unwrap();
            assert_eq!(json, expected);
            let back: ChannelType = serde_json::from_str(&json).unwrap();
            assert_eq!(back, t);
        }
    }

    #[test]
    fn test_discord_message_roundtrip() {
        let msg = DiscordMessage {
            id: 1,
            channel_id: 100,
            guild_id: Some(200),
            author: DiscordUser {
                id: 42,
                username: "alice".to_string(),
                global_name: None,
                bot: false,
            },
            content: "Hello world".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            edited_timestamp: None,
            attachments: vec![],
            embeds: vec![],
            referenced_message_id: None,
            referenced_message_content: None,
        };
        roundtrip(&msg);

        // With reply context
        let reply_msg = DiscordMessage {
            referenced_message_id: Some(999),
            referenced_message_content: Some("the original message".to_string()),
            ..msg.clone()
        };
        roundtrip(&reply_msg);
    }

    #[test]
    fn test_command_option_value_serde() {
        roundtrip(&CommandOptionValue::String("hello".to_string()));
        roundtrip(&CommandOptionValue::Integer(42));
        roundtrip(&CommandOptionValue::Boolean(true));
        roundtrip(&CommandOptionValue::User(99));
    }

    #[test]
    fn test_embed_roundtrip() {
        let embed = Embed {
            title: Some("Title".to_string()),
            description: Some("Desc".to_string()),
            url: None,
            fields: vec![EmbedField {
                name: "Field".to_string(),
                value: "Value".to_string(),
                inline: false,
            }],
            color: Some(0xFF0000),
        };
        roundtrip(&embed);
    }
}
