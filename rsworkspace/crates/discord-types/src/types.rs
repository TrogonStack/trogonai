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

/// Embed author
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbedAuthor {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
}

/// Embed footer
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbedFooter {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
}

/// Embed image or thumbnail (just a URL)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbedMedia {
    pub url: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<EmbedAuthor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footer: Option<EmbedFooter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<EmbedMedia>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<EmbedMedia>,
    /// ISO 8601 timestamp string
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
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

/// Discord role
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscordRole {
    pub id: u64,
    pub name: String,
    pub color: u32,
    pub hoist: bool,
    pub position: i64,
    /// Permission bitmask as decimal string
    pub permissions: String,
    pub mentionable: bool,
}

/// Voice state snapshot
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceState {
    pub user_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    pub self_mute: bool,
    pub self_deaf: bool,
}

/// Modal text input value
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModalInput {
    pub custom_id: String,
    pub value: String,
}

/// Button style
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ButtonStyle {
    Primary,
    Secondary,
    Success,
    Danger,
    /// Link buttons open a URL instead of firing an interaction
    Link,
}

/// A button component
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Button {
    pub style: ButtonStyle,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<Emoji>,
    /// Required for non-Link buttons (used as the interaction custom_id)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_id: Option<String>,
    /// Required for Link buttons
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

/// One option in a string select menu
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SelectOption {
    pub label: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<Emoji>,
    #[serde(default)]
    pub default: bool,
}

/// A string select menu component
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StringSelectMenu {
    pub custom_id: String,
    #[serde(default)]
    pub options: Vec<SelectOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default = "default_min_values")]
    pub min_values: u8,
    #[serde(default = "default_max_values")]
    pub max_values: u8,
    #[serde(default)]
    pub disabled: bool,
}

fn default_min_values() -> u8 {
    1
}
fn default_max_values() -> u8 {
    1
}

/// A component inside an action row
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionRowComponent {
    Button(Button),
    StringSelect(StringSelectMenu),
    UserSelect(StringSelectMenu),
    RoleSelect(StringSelectMenu),
    ChannelSelect(StringSelectMenu),
}

/// An action row — holds up to 5 buttons or 1 select menu
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActionRow {
    #[serde(default)]
    pub components: Vec<ActionRowComponent>,
}

/// A file to attach to a message via local filesystem path on the bot host
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttachedFile {
    pub filename: String,
    /// Absolute path on the bot's filesystem
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Bot activity type (for rich presence)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivityType {
    Playing,
    Streaming,
    Listening,
    Watching,
    Custom,
    Competing,
}

/// Bot activity/game shown in presence
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BotActivity {
    pub kind: ActivityType,
    pub name: String,
    /// Stream URL — only used when kind = Streaming
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Bot online status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BotStatus {
    Online,
    Idle,
    DoNotDisturb,
    Invisible,
}

/// A Discord message returned by a fetch request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchedMessage {
    pub id: u64,
    pub channel_id: u64,
    pub author: DiscordUser,
    pub content: String,
    pub timestamp: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub embeds: Vec<Embed>,
}

/// A guild member returned by a fetch request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchedMember {
    pub user: DiscordUser,
    pub guild_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nick: Option<String>,
    #[serde(default)]
    pub roles: Vec<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub joined_at: Option<String>,
}

/// A guild returned by a fetch request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchedGuild {
    pub id: u64,
    pub name: String,
    pub member_count: u64,
}

/// A channel returned by a fetch request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchedChannel {
    pub id: u64,
    pub channel_type: ChannelType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
}

/// An invite returned by a fetch request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchedInvite {
    pub code: String,
    pub channel_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inviter_id: Option<u64>,
    pub uses: u64,
    pub max_uses: u64,
    pub max_age_secs: u64,
    pub temporary: bool,
}

/// A guild role returned by a fetch request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchedRole {
    pub id: u64,
    pub name: String,
    pub color: u32,
    pub hoist: bool,
    pub mentionable: bool,
    /// Permissions bitfield
    pub permissions: u64,
}

/// A ban entry returned by a fetch request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchedBan {
    pub user_id: u64,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Minimal sticker info from a guild stickers update
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StickerInfo {
    pub id: u64,
    pub name: String,
}

/// Simplified audit log entry
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditLogEntryInfo {
    pub id: u64,
    pub user_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<u64>,
    /// Numeric action type code
    pub action_type: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// User who RSVPed to a scheduled event
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScheduledEventUserInfo {
    pub event_id: u64,
    pub user: DiscordUser,
}

/// Discord voice region
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceRegionInfo {
    pub id: String,
    pub name: String,
    pub optimal: bool,
    pub deprecated: bool,
}

/// Minimal application info
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppInfo {
    pub id: u64,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<u64>,
}

/// Minimal soundboard sound info
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SoundInfo {
    pub id: u64,
    pub name: String,
    pub volume: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    pub available: bool,
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
            author: None,
            footer: None,
            image: None,
            thumbnail: None,
            timestamp: None,
        };
        roundtrip(&embed);
    }

    #[test]
    fn test_discord_role_roundtrip() {
        let role = DiscordRole {
            id: 111,
            name: "Moderator".to_string(),
            color: 0x00FF00,
            hoist: true,
            position: 3,
            permissions: "8".to_string(),
            mentionable: true,
        };
        roundtrip(&role);
    }

    #[test]
    fn test_discord_role_optional_fields_omitted() {
        // permissions bitmask must serialize as string, not number
        let role = DiscordRole {
            id: 1,
            name: "everyone".to_string(),
            color: 0,
            hoist: false,
            position: 0,
            permissions: "104320065".to_string(),
            mentionable: false,
        };
        let json = serde_json::to_string(&role).unwrap();
        // permissions must be a JSON string
        assert!(
            json.contains("\"104320065\""),
            "permissions must be a string, got: {}",
            json
        );
    }

    #[test]
    fn test_voice_state_roundtrip() {
        let state = VoiceState {
            user_id: 42,
            channel_id: Some(500),
            guild_id: Some(200),
            self_mute: true,
            self_deaf: false,
        };
        roundtrip(&state);
    }

    #[test]
    fn test_voice_state_no_channel_omitted() {
        let state = VoiceState {
            user_id: 42,
            channel_id: None,
            guild_id: None,
            self_mute: false,
            self_deaf: false,
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(
            !json.contains("channel_id"),
            "channel_id must be omitted when None"
        );
        assert!(!json.contains("guild_id"), "guild_id must be omitted when None");
        roundtrip(&state);
    }

    #[test]
    fn test_modal_input_roundtrip() {
        let input = ModalInput {
            custom_id: "name_field".to_string(),
            value: "Alice".to_string(),
        };
        roundtrip(&input);
    }
}
