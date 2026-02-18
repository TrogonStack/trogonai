//! Commands sent from NATS agents to the Discord bot

use serde::{Deserialize, Serialize};

use crate::types::{ActionRow, AttachedFile, BotActivity, BotStatus, ChannelType, Embed, ModalInput};

/// Send a new message to a channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SendMessageCommand {
    pub channel_id: u64,
    pub content: String,
    #[serde(default)]
    pub embeds: Vec<Embed>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<u64>,
    /// Local files to attach (paths resolved on the bot host)
    #[serde(default)]
    pub files: Vec<AttachedFile>,
    /// Interactive action rows (buttons / select menus)
    #[serde(default)]
    pub components: Vec<ActionRow>,
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
    #[serde(default)]
    pub components: Vec<ActionRow>,
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
    /// Interactive action rows (buttons / select menus)
    #[serde(default)]
    pub components: Vec<ActionRow>,
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
    /// Local files to attach (paths resolved on the bot host)
    #[serde(default)]
    pub files: Vec<AttachedFile>,
    /// Interactive action rows (buttons / select menus)
    #[serde(default)]
    pub components: Vec<ActionRow>,
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

/// Set the bot's online status and activity (requires shard access on the bot side)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SetBotPresenceCommand {
    pub status: BotStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<BotActivity>,
}

/// Fetch recent messages from a channel (request-reply over NATS)
///
/// The bot replies with `Vec<FetchedMessage>` serialized as JSON to the
/// NATS reply inbox. Use `client.request(subject, payload)` on the agent side.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchMessagesCommand {
    pub channel_id: u64,
    /// Number of messages to fetch (1-100, default 20)
    #[serde(default = "default_fetch_limit")]
    pub limit: u8,
    /// Fetch messages before this message ID (for pagination)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_id: Option<u64>,
}

fn default_fetch_limit() -> u8 {
    20
}

/// Fetch a single guild member (request-reply over NATS)
///
/// The bot replies with `FetchedMember` serialized as JSON to the NATS reply inbox.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchMemberCommand {
    pub guild_id: u64,
    pub user_id: u64,
}

/// Unban a user from a guild
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnbanUserCommand {
    pub guild_id: u64,
    pub user_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Set or clear a guild member's nickname
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildMemberNickCommand {
    pub guild_id: u64,
    pub user_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nick: Option<String>,
}

/// Create a webhook in a channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateWebhookCommand {
    pub channel_id: u64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

/// Execute (send a message via) a webhook
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecuteWebhookCommand {
    pub webhook_id: u64,
    pub webhook_token: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

/// Delete a webhook
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteWebhookCommand {
    pub webhook_id: u64,
    pub webhook_token: String,
}

/// Move a guild member to a different voice channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceMoveCommand {
    pub guild_id: u64,
    pub user_id: u64,
    pub channel_id: u64,
}

/// Disconnect a guild member from voice
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceDisconnectCommand {
    pub guild_id: u64,
    pub user_id: u64,
}

/// Create an invite for a channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateInviteCommand {
    pub channel_id: u64,
    /// Expiry in seconds (0 = never expires)
    #[serde(default)]
    pub max_age_secs: u64,
    /// Maximum uses (0 = unlimited)
    #[serde(default)]
    pub max_uses: u32,
    /// Whether the invite grants temporary membership
    #[serde(default)]
    pub temporary: bool,
    /// Whether to always create a unique invite URL
    #[serde(default)]
    pub unique: bool,
}

/// Revoke (delete) an invite by its code
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RevokeInviteCommand {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Create a custom emoji in a guild
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateEmojiCommand {
    pub guild_id: u64,
    pub name: String,
    /// Base64 data URI (e.g. "data:image/png;base64,...")
    pub image_data: String,
}

/// Delete a custom emoji from a guild
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteEmojiCommand {
    pub guild_id: u64,
    pub emoji_id: u64,
}

/// Create a guild scheduled event
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateScheduledEventCommand {
    pub guild_id: u64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// ISO 8601 start time
    pub start_time: String,
    /// ISO 8601 end time (required for external events)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_time: Option<String>,
    /// Voice/stage channel ID for in-channel events
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<u64>,
    /// Location string for external events
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_location: Option<String>,
}

/// Delete (cancel) a guild scheduled event
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteScheduledEventCommand {
    pub guild_id: u64,
    pub event_id: u64,
}

/// Remove all reactions from a message (optionally filtered to one emoji)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RemoveAllReactionsCommand {
    pub channel_id: u64,
    pub message_id: u64,
    /// If set, removes only reactions for this emoji (unicode char or `name:id`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
}

/// Crosspost (publish) an announcement-channel message to follower channels
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CrosspostMessageCommand {
    pub channel_id: u64,
    pub message_id: u64,
}

/// Create a new post (thread + first message) in a forum channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateForumPostCommand {
    pub channel_id: u64,
    pub name: String,
    pub content: String,
    /// Applied forum tag IDs
    #[serde(default)]
    pub tags: Vec<u64>,
}

/// Add a member to a thread
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AddThreadMemberCommand {
    pub thread_id: u64,
    pub user_id: u64,
}

/// Remove a member from a thread
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RemoveThreadMemberCommand {
    pub thread_id: u64,
    pub user_id: u64,
}

/// Edit a guild role's name, color, hoist, or mentionable flag
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EditRoleCommand {
    pub guild_id: u64,
    pub role_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hoist: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mentionable: Option<bool>,
}

/// Start a stage instance in a stage voice channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateStageInstanceCommand {
    pub channel_id: u64,
    pub topic: String,
}

/// End (delete) a stage instance
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteStageInstanceCommand {
    pub channel_id: u64,
}

/// Edit a webhook's name or target channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EditWebhookCommand {
    pub webhook_id: u64,
    pub webhook_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<u64>,
}

/// Open a DM channel with a user (request-reply: returns channel_id as `Option<u64>`)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateDmChannelCommand {
    pub user_id: u64,
}

/// Fetch guild info (request-reply: returns `Option<FetchedGuild>`)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchGuildCommand {
    pub guild_id: u64,
}

/// Fetch channel info (request-reply: returns `Option<FetchedChannel>`)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchChannelCommand {
    pub channel_id: u64,
}

/// Fetch active invites for a channel (request-reply: returns `Vec<FetchedInvite>`)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchInvitesCommand {
    pub channel_id: u64,
}

/// Edit a guild scheduled event
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EditScheduledEventCommand {
    pub guild_id: u64,
    pub event_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// ISO 8601 start time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<String>,
}

/// Rename a custom emoji
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EditEmojiCommand {
    pub guild_id: u64,
    pub emoji_id: u64,
    pub name: String,
}

/// Fetch guild members (request-reply: returns `Vec<FetchedMember>`)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchGuildMembersCommand {
    pub guild_id: u64,
    /// Max number of members to return (1–1000, default 100)
    #[serde(default)]
    pub limit: Option<u64>,
    /// Fetch members after this user ID (for pagination)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_id: Option<u64>,
}

/// Fetch all channels in a guild (request-reply: returns `Vec<FetchedChannel>`)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchGuildChannelsCommand {
    pub guild_id: u64,
}

/// Fetch pinned messages in a channel (request-reply: returns `Vec<FetchedMessage>`)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchPinnedMessagesCommand {
    pub channel_id: u64,
}

/// Fetch all roles in a guild (request-reply: returns `Vec<FetchedRole>`)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchRolesCommand {
    pub guild_id: u64,
}

/// Edit the original response to an interaction
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EditInteractionResponseCommand {
    pub interaction_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default)]
    pub embeds: Vec<crate::types::Embed>,
}

/// Delete the original response to an interaction
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteInteractionResponseCommand {
    pub interaction_token: String,
}

/// Set (create or update) a permission overwrite on a channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SetChannelPermissionsCommand {
    pub channel_id: u64,
    /// ID of the role or user
    pub target_id: u64,
    /// "role" or "member"
    pub target_type: String,
    /// Allowed permissions bitfield
    pub allow: u64,
    /// Denied permissions bitfield
    pub deny: u64,
}

/// Delete a permission overwrite from a channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteChannelPermissionsCommand {
    pub channel_id: u64,
    /// ID of the role or user whose overwrite to remove
    pub target_id: u64,
}

/// Kick inactive members from a guild (prune)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PruneMembersCommand {
    pub guild_id: u64,
    /// Members inactive for this many days are pruned (1–30)
    pub days: u8,
}

/// Fetch audit log entries for a guild (request-reply: returns `Vec<AuditLogEntryInfo>`)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchAuditLogCommand {
    pub guild_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u8>,
    /// Return entries before this entry ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_id: Option<u64>,
    /// Filter by this user ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u64>,
}

/// Fetch users subscribed to a scheduled event (request-reply: returns `Vec<ScheduledEventUserInfo>`)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchScheduledEventUsersCommand {
    pub guild_id: u64,
    pub event_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

/// Edit a guild sticker's metadata
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EditStickerCommand {
    pub guild_id: u64,
    pub sticker_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Delete a guild sticker
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteStickerCommand {
    pub guild_id: u64,
    pub sticker_id: u64,
}

/// Delete a guild integration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteIntegrationCommand {
    pub guild_id: u64,
    pub integration_id: u64,
}

/// Sync a guild integration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncIntegrationCommand {
    pub guild_id: u64,
    pub integration_id: u64,
}

/// Fetch available voice regions (request-reply: returns `Vec<VoiceRegionInfo>`)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchVoiceRegionsCommand {}

/// Fetch the bot's application info (request-reply: returns `Option<AppInfo>`)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchApplicationInfoCommand {}

/// Approve a DM pairing request by its code (admin → bot)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairingApproveCommand {
    /// The 6-character pairing code shared by the user
    pub code: String,
}

/// Reject a DM pairing request by its code (admin → bot)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairingRejectCommand {
    /// The 6-character pairing code shared by the user
    pub code: String,
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
            files: vec![],
            components: vec![],
        });
    }

    #[test]
    fn test_send_message_no_reply() {
        roundtrip(&SendMessageCommand {
            channel_id: 100,
            content: "Hello".to_string(),
            embeds: vec![],
            reply_to_message_id: None,
            files: vec![],
            components: vec![],
        });
    }

    #[test]
    fn test_edit_message_command_roundtrip() {
        roundtrip(&EditMessageCommand {
            channel_id: 100,
            message_id: 1,
            content: Some("Updated".to_string()),
            embeds: vec![],
            components: vec![],
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
            components: vec![],
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
            files: vec![],
            components: vec![],
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
            files: vec![],
            components: vec![],
        });
        // session_id must be omitted when None
        let cmd = InteractionFollowupCommand {
            interaction_token: "tok".to_string(),
            content: None,
            embeds: vec![],
            ephemeral: false,
            session_id: None,
            files: vec![],
            components: vec![],
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
