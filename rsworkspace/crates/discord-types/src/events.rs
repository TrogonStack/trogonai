//! Events published from Discord bot to NATS

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::{
    AppInfo, AuditLogEntryInfo, CommandOption, ComponentType, DiscordChannel, DiscordGuild,
    DiscordMember, DiscordMessage, DiscordRole, DiscordUser, Embed, Emoji, FetchedMember,
    ModalInput, ScheduledEventUserInfo, SoundInfo, StickerInfo, VoiceRegionInfo, VoiceState,
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
    /// PluralKit member ID if this message was proxied through PluralKit
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluralkit_member_id: Option<String>,
    /// PluralKit member display name if proxied
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluralkit_member_name: Option<String>,
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
    /// Selected values for select-menu components. Empty for button interactions.
    #[serde(default)]
    pub values: Vec<String>,
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

/// User started typing in a channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TypingStartEvent {
    pub metadata: EventMetadata,
    pub user_id: u64,
    pub channel_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
}

/// A user's voice state changed
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceStateUpdateEvent {
    pub metadata: EventMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_channel_id: Option<u64>,
    pub new_state: VoiceState,
}

/// Bot joined or a guild became available
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildCreateEvent {
    pub metadata: EventMetadata,
    pub guild: DiscordGuild,
    pub member_count: u64,
}

/// A guild was updated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildUpdateEvent {
    pub metadata: EventMetadata,
    pub guild: DiscordGuild,
}

/// Bot left or a guild became unavailable
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildDeleteEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub unavailable: bool,
}

/// A channel was created in a guild
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelCreateEvent {
    pub metadata: EventMetadata,
    pub channel: DiscordChannel,
}

/// A channel was updated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelUpdateEvent {
    pub metadata: EventMetadata,
    pub channel: DiscordChannel,
}

/// A channel was deleted
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelDeleteEvent {
    pub metadata: EventMetadata,
    pub channel_id: u64,
    pub guild_id: u64,
}

/// A role was created in a guild
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoleCreateEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub role: DiscordRole,
}

/// A role was updated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoleUpdateEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub role: DiscordRole,
}

/// A role was deleted
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoleDeleteEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub role_id: u64,
}

/// A user's presence (online status) changed
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PresenceUpdateEvent {
    pub metadata: EventMetadata,
    pub user_id: u64,
    pub guild_id: u64,
    pub status: String,
}

/// A modal was submitted by a user
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModalSubmitEvent {
    pub metadata: EventMetadata,
    pub interaction_id: u64,
    pub interaction_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    pub channel_id: u64,
    pub user: DiscordUser,
    pub custom_id: String,
    #[serde(default)]
    pub inputs: Vec<ModalInput>,
}

/// An autocomplete interaction was triggered
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutocompleteEvent {
    pub metadata: EventMetadata,
    pub interaction_id: u64,
    pub interaction_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    pub channel_id: u64,
    pub user: DiscordUser,
    pub command_name: String,
    pub focused_option: String,
    pub current_value: String,
}

/// The bot connected and is ready
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BotReadyEvent {
    pub metadata: EventMetadata,
    pub bot_user: DiscordUser,
    pub guild_count: u64,
}

/// Multiple messages were bulk-deleted from a channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageBulkDeleteEvent {
    pub metadata: EventMetadata,
    pub channel_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    #[serde(default)]
    pub message_ids: Vec<u64>,
}

/// A guild member's attributes were updated (nick, roles, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildMemberUpdateEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub user: DiscordUser,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nick: Option<String>,
    #[serde(default)]
    pub roles: Vec<u64>,
}

/// A thread channel was created
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadCreatedEvent {
    pub metadata: EventMetadata,
    pub thread_id: u64,
    pub guild_id: u64,
    /// Parent channel ID
    pub parent_id: u64,
    pub name: String,
    pub archived: bool,
    pub locked: bool,
    pub auto_archive_duration: u64,
}

/// A thread channel was updated (name, archived, locked, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadUpdatedEvent {
    pub metadata: EventMetadata,
    pub thread_id: u64,
    pub guild_id: u64,
    pub parent_id: u64,
    pub name: String,
    pub archived: bool,
    pub locked: bool,
}

/// A thread channel was deleted
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadDeletedEvent {
    pub metadata: EventMetadata,
    pub thread_id: u64,
    pub guild_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<u64>,
}

/// One or more users joined a thread
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadMemberAddEvent {
    pub metadata: EventMetadata,
    pub thread_id: u64,
    pub guild_id: u64,
    /// User IDs that joined the thread
    pub user_ids: Vec<u64>,
}

/// One or more users left (or were removed from) a thread
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadMemberRemoveEvent {
    pub metadata: EventMetadata,
    pub thread_id: u64,
    pub guild_id: u64,
    /// User IDs that left the thread
    pub user_ids: Vec<u64>,
}

/// An invite was created
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InviteCreateEvent {
    pub metadata: EventMetadata,
    pub code: String,
    pub channel_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inviter_id: Option<u64>,
    /// Expiry in seconds (0 = never)
    pub max_age_secs: u64,
    /// Maximum uses (0 = unlimited)
    pub max_uses: u64,
    pub temporary: bool,
}

/// An invite was deleted or expired
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InviteDeleteEvent {
    pub metadata: EventMetadata,
    pub code: String,
    pub channel_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
}

/// A stage instance was created in a stage channel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StageInstanceCreateEvent {
    pub metadata: EventMetadata,
    pub stage_id: u64,
    pub guild_id: u64,
    pub channel_id: u64,
    pub topic: String,
}

/// A stage instance's topic was updated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StageInstanceUpdateEvent {
    pub metadata: EventMetadata,
    pub stage_id: u64,
    pub guild_id: u64,
    pub channel_id: u64,
    pub topic: String,
}

/// A stage instance ended (deleted)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StageInstanceDeleteEvent {
    pub metadata: EventMetadata,
    pub stage_id: u64,
    pub guild_id: u64,
    pub channel_id: u64,
}

/// A user was banned from a guild
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildBanAddEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub user: DiscordUser,
}

/// A user was unbanned from a guild
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildBanRemoveEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub user: DiscordUser,
}

/// Guild custom emoji list changed
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildEmojisUpdateEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub emojis: Vec<Emoji>,
}

/// A guild scheduled event was created
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildScheduledEventCreateEvent {
    pub metadata: EventMetadata,
    pub event_id: u64,
    pub guild_id: u64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub start_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<u64>,
}

/// A guild scheduled event was updated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildScheduledEventUpdateEvent {
    pub metadata: EventMetadata,
    pub event_id: u64,
    pub guild_id: u64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub start_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<u64>,
}

/// A guild scheduled event was deleted or cancelled
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildScheduledEventDeleteEvent {
    pub metadata: EventMetadata,
    pub event_id: u64,
    pub guild_id: u64,
    pub name: String,
}

/// A channel's pinned messages list was updated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelPinsUpdateEvent {
    pub metadata: EventMetadata,
    pub channel_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_pin_timestamp: Option<String>,
}

/// All reactions were removed from a message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReactionRemoveAllEvent {
    pub metadata: EventMetadata,
    pub channel_id: u64,
    pub message_id: u64,
}

/// A guild's webhooks were updated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebhooksUpdateEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub channel_id: u64,
}

/// Guild sticker list changed
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildStickersUpdateEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub stickers: Vec<StickerInfo>,
}

/// Guild integrations were updated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildIntegrationsUpdateEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
}

/// A user subscribed to a guild scheduled event
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildScheduledEventUserAddEvent {
    pub metadata: EventMetadata,
    pub event_id: u64,
    pub user_id: u64,
    pub guild_id: u64,
}

/// A user unsubscribed from a guild scheduled event
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildScheduledEventUserRemoveEvent {
    pub metadata: EventMetadata,
    pub event_id: u64,
    pub user_id: u64,
    pub guild_id: u64,
}

/// A category channel was created
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CategoryCreateEvent {
    pub metadata: EventMetadata,
    pub channel: DiscordChannel,
}

/// A category channel was deleted
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CategoryDeleteEvent {
    pub metadata: EventMetadata,
    pub channel: DiscordChannel,
}

/// Bot reconnected to the gateway
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BotResumeEvent {
    pub metadata: EventMetadata,
}

/// The current user (bot) profile was updated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserUpdateEvent {
    pub metadata: EventMetadata,
    pub user_id: u64,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_name: Option<String>,
}

/// Bulk guild members chunk received
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildMembersChunkEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub chunk_index: u32,
    pub chunk_count: u32,
    pub members: Vec<FetchedMember>,
}

/// Voice server update (used for voice connections)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceServerUpdateEvent {
    pub metadata: EventMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    pub token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

/// An audit log entry was created
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuildAuditLogEntryEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub entry: AuditLogEntryInfo,
}

/// Thread list was synced
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadListSyncEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_ids: Option<Vec<u64>>,
    pub thread_count: u32,
}

/// A thread member was updated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadMemberUpdateEvent {
    pub metadata: EventMetadata,
    pub thread_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    pub user_id: u64,
}

/// An integration was created
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IntegrationCreateEvent {
    pub metadata: EventMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    pub integration_id: u64,
    pub name: String,
    pub kind: String,
    pub enabled: bool,
}

/// An integration was updated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IntegrationUpdateEvent {
    pub metadata: EventMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    pub integration_id: u64,
    pub name: String,
    pub kind: String,
    pub enabled: bool,
}

/// An integration was deleted
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IntegrationDeleteEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub integration_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application_id: Option<u64>,
}

/// Voice channel status was updated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceChannelStatusUpdateEvent {
    pub metadata: EventMetadata,
    pub channel_id: u64,
    pub guild_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_status: Option<String>,
}

/// An auto-moderation rule was created
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutoModRuleCreateEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub rule_id: u64,
    pub name: String,
    pub enabled: bool,
}

/// An auto-moderation rule was updated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutoModRuleUpdateEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub rule_id: u64,
    pub name: String,
    pub enabled: bool,
}

/// An auto-moderation rule was deleted
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutoModRuleDeleteEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub rule_id: u64,
    pub name: String,
}

/// An auto-moderation action was executed
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutoModActionExecutionEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub rule_id: u64,
    pub user_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_keyword: Option<String>,
}

/// Application command permissions were updated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandPermissionsUpdateEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub command_id: u64,
    pub application_id: u64,
}

/// An entitlement was created (monetization)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntitlementCreateEvent {
    pub metadata: EventMetadata,
    pub entitlement_id: u64,
    pub sku_id: u64,
    pub application_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
}

/// An entitlement was updated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntitlementUpdateEvent {
    pub metadata: EventMetadata,
    pub entitlement_id: u64,
    pub sku_id: u64,
    pub application_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
}

/// An entitlement was deleted
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntitlementDeleteEvent {
    pub metadata: EventMetadata,
    pub entitlement_id: u64,
    pub sku_id: u64,
    pub application_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
}

/// A user voted on a message poll
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PollVoteAddEvent {
    pub metadata: EventMetadata,
    pub user_id: u64,
    pub channel_id: u64,
    pub message_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    pub answer_id: u64,
}

/// A user removed their vote from a message poll
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PollVoteRemoveEvent {
    pub metadata: EventMetadata,
    pub user_id: u64,
    pub channel_id: u64,
    pub message_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    pub answer_id: u64,
}

/// A specific emoji's reactions were removed from a message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReactionRemoveEmojiEvent {
    pub metadata: EventMetadata,
    pub channel_id: u64,
    pub message_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji_id: Option<u64>,
}

/// Fetch audit log result (response to FetchAuditLogCommand)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchAuditLogResult {
    pub entries: Vec<AuditLogEntryInfo>,
}

/// Fetch scheduled event users result (response to FetchScheduledEventUsersCommand)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchScheduledEventUsersResult {
    pub users: Vec<ScheduledEventUserInfo>,
}

/// Fetch voice regions result (response to FetchVoiceRegionsCommand)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchVoiceRegionsResult {
    pub regions: Vec<VoiceRegionInfo>,
}

/// Fetch application info result (response to FetchApplicationInfoCommand)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FetchApplicationInfoResult {
    pub app: Option<AppInfo>,
}

/// Bulk presence replace — sent by Discord when joining large guilds (count only, not full data)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PresenceReplaceEvent {
    pub metadata: EventMetadata,
    pub count: u32,
}

/// Shard connection stage changed
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShardStageUpdateEvent {
    pub metadata: EventMetadata,
    pub shard_id: u64,
    pub old: String,
    pub new: String,
}

/// All soundboard sounds for a guild received
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SoundboardSoundsEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub sounds: Vec<SoundInfo>,
}

/// New soundboard sound created in a guild
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SoundboardSoundCreateEvent {
    pub metadata: EventMetadata,
    pub sound: SoundInfo,
}

/// Soundboard sound updated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SoundboardSoundUpdateEvent {
    pub metadata: EventMetadata,
    pub sound: SoundInfo,
}

/// Multiple soundboard sounds updated for a guild
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SoundboardSoundsUpdateEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub sounds: Vec<SoundInfo>,
}

/// Soundboard sound deleted
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SoundboardSoundDeleteEvent {
    pub metadata: EventMetadata,
    pub guild_id: u64,
    pub sound_id: u64,
}

/// HTTP rate limit hit while processing a Discord API request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RatelimitEvent {
    pub metadata: EventMetadata,
    pub path: String,
    pub timeout_ms: u64,
    pub global: bool,
}

/// A user has requested DM pairing and is awaiting admin approval
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairingRequestedEvent {
    pub metadata: EventMetadata,
    pub user_id: u64,
    pub username: String,
    pub code: String,
    /// Unix timestamp (seconds) when the code expires
    pub expires_at: u64,
}

/// A user's DM pairing request was approved
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairingApprovedEvent {
    pub metadata: EventMetadata,
    pub user_id: u64,
}

/// A user's DM pairing request was rejected
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairingRejectedEvent {
    pub metadata: EventMetadata,
    pub user_id: u64,
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
            pluralkit_member_id: None,
            pluralkit_member_name: None,
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
            values: vec![],
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

    fn test_channel() -> DiscordChannel {
        DiscordChannel {
            id: 300,
            channel_type: crate::types::ChannelType::GuildText,
            guild_id: Some(200),
            name: Some("general".to_string()),
        }
    }

    fn test_guild() -> DiscordGuild {
        DiscordGuild {
            id: 200,
            name: "Test Server".to_string(),
        }
    }

    fn test_role() -> DiscordRole {
        DiscordRole {
            id: 111,
            name: "Moderator".to_string(),
            color: 0xFF0000,
            hoist: true,
            position: 2,
            permissions: "8".to_string(),
            mentionable: true,
        }
    }

    fn test_voice_state() -> VoiceState {
        VoiceState {
            user_id: 42,
            channel_id: Some(500),
            guild_id: Some(200),
            self_mute: false,
            self_deaf: false,
        }
    }

    #[test]
    fn test_typing_start_event_roundtrip() {
        roundtrip(&TypingStartEvent {
            metadata: test_meta(),
            user_id: 42,
            channel_id: 100,
            guild_id: Some(200),
        });
    }

    #[test]
    fn test_typing_start_event_dm_no_guild() {
        let event = TypingStartEvent {
            metadata: test_meta(),
            user_id: 42,
            channel_id: 100,
            guild_id: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(
            !json.contains("guild_id"),
            "guild_id must be omitted in DMs"
        );
        roundtrip(&event);
    }

    #[test]
    fn test_voice_state_update_event_roundtrip() {
        roundtrip(&VoiceStateUpdateEvent {
            metadata: test_meta(),
            guild_id: Some(200),
            old_channel_id: Some(400),
            new_state: test_voice_state(),
        });
    }

    #[test]
    fn test_voice_state_update_event_join() {
        // Joining a channel: old_channel_id = None
        let event = VoiceStateUpdateEvent {
            metadata: test_meta(),
            guild_id: Some(200),
            old_channel_id: None,
            new_state: test_voice_state(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(
            !json.contains("old_channel_id"),
            "old_channel_id must be omitted when None"
        );
        roundtrip(&event);
    }

    #[test]
    fn test_guild_create_event_roundtrip() {
        roundtrip(&GuildCreateEvent {
            metadata: test_meta(),
            guild: test_guild(),
            member_count: 42,
        });
    }

    #[test]
    fn test_guild_update_event_roundtrip() {
        roundtrip(&GuildUpdateEvent {
            metadata: test_meta(),
            guild: test_guild(),
        });
    }

    #[test]
    fn test_guild_delete_event_roundtrip() {
        roundtrip(&GuildDeleteEvent {
            metadata: test_meta(),
            guild_id: 200,
            unavailable: true,
        });
    }

    #[test]
    fn test_channel_create_event_roundtrip() {
        roundtrip(&ChannelCreateEvent {
            metadata: test_meta(),
            channel: test_channel(),
        });
    }

    #[test]
    fn test_channel_update_event_roundtrip() {
        roundtrip(&ChannelUpdateEvent {
            metadata: test_meta(),
            channel: test_channel(),
        });
    }

    #[test]
    fn test_channel_delete_event_roundtrip() {
        roundtrip(&ChannelDeleteEvent {
            metadata: test_meta(),
            channel_id: 300,
            guild_id: 200,
        });
    }

    #[test]
    fn test_role_create_event_roundtrip() {
        roundtrip(&RoleCreateEvent {
            metadata: test_meta(),
            guild_id: 200,
            role: test_role(),
        });
    }

    #[test]
    fn test_role_update_event_roundtrip() {
        roundtrip(&RoleUpdateEvent {
            metadata: test_meta(),
            guild_id: 200,
            role: test_role(),
        });
    }

    #[test]
    fn test_role_delete_event_roundtrip() {
        roundtrip(&RoleDeleteEvent {
            metadata: test_meta(),
            guild_id: 200,
            role_id: 111,
        });
    }

    #[test]
    fn test_presence_update_event_roundtrip() {
        roundtrip(&PresenceUpdateEvent {
            metadata: test_meta(),
            user_id: 42,
            guild_id: 200,
            status: "online".to_string(),
        });
    }

    #[test]
    fn test_modal_submit_event_roundtrip() {
        use crate::types::ModalInput;
        roundtrip(&ModalSubmitEvent {
            metadata: test_meta(),
            interaction_id: 7777,
            interaction_token: "modal-tok".to_string(),
            guild_id: Some(200),
            channel_id: 100,
            user: test_user(),
            custom_id: "my_modal".to_string(),
            inputs: vec![ModalInput {
                custom_id: "name".to_string(),
                value: "Alice".to_string(),
            }],
        });
    }

    #[test]
    fn test_modal_submit_event_dm_no_guild() {
        let event = ModalSubmitEvent {
            metadata: test_meta(),
            interaction_id: 7777,
            interaction_token: "tok".to_string(),
            guild_id: None,
            channel_id: 100,
            user: test_user(),
            custom_id: "my_modal".to_string(),
            inputs: vec![],
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(
            !json.contains("guild_id"),
            "guild_id must be omitted in DMs"
        );
        roundtrip(&event);
    }

    #[test]
    fn test_autocomplete_event_roundtrip() {
        roundtrip(&AutocompleteEvent {
            metadata: test_meta(),
            interaction_id: 6666,
            interaction_token: "ac-tok".to_string(),
            guild_id: Some(200),
            channel_id: 100,
            user: test_user(),
            command_name: "ask".to_string(),
            focused_option: "question".to_string(),
            current_value: "how".to_string(),
        });
    }

    #[test]
    fn test_bot_ready_event_roundtrip() {
        roundtrip(&BotReadyEvent {
            metadata: test_meta(),
            bot_user: DiscordUser {
                id: 999,
                username: "MyBot".to_string(),
                global_name: Some("My Bot".to_string()),
                bot: true,
            },
            guild_count: 5,
        });
    }
}
