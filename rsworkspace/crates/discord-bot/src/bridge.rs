//! Bridge between Discord (serenity) and NATS
//!
//! Converts serenity events into discord-types events and publishes them to NATS.

#[path = "bridge_tests.rs"]
mod bridge_tests;

#[path = "bridge_nats_tests.rs"]
mod bridge_nats_tests;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use discord_nats::{subjects, MessagePublisher};
use discord_types::{
    events::*,
    session::{member_session_id, session_id},
    types::{
        Attachment, AuditLogEntryInfo, ChannelType, CommandOption, CommandOptionValue,
        ComponentType, DiscordChannel, DiscordGuild, DiscordMember, DiscordMessage, DiscordRole,
        DiscordUser, Embed, EmbedAuthor, EmbedField, EmbedFooter, EmbedMedia, Emoji, FetchedMember,
        ModalInput, SoundInfo, StickerInfo, VoiceState as BridgeVoiceState,
    },
    AccessConfig, DmPolicy,
};

// ── Pairing state ─────────────────────────────────────────────────────────────

/// A single pending pairing request.
struct PendingPairing {
    code: String,
    channel_id: u64,
    expires_at: u64, // Unix seconds
}

/// Runtime state for DM pairing flow.
///
/// Shared between the event bridge (inbound DMs) and the outbound processor
/// (approve/reject commands from admins).
pub struct PairingState {
    pending: Mutex<HashMap<u64, PendingPairing>>, // user_id → pending
    approved: Mutex<HashSet<u64>>,
}

impl PairingState {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            approved: Mutex::new(HashSet::new()),
        }
    }

    /// Returns true if the user has been approved to send DMs.
    pub fn is_paired(&self, user_id: u64) -> bool {
        self.approved.lock().unwrap().contains(&user_id)
    }

    /// Begin a pairing session for `user_id`.
    ///
    /// Returns `(code, expires_at)`. If a pending entry already exists it is
    /// reused so the user always receives the same code within one session.
    /// Returns `None` when the user is already approved.
    pub fn start_pairing(&self, user_id: u64, channel_id: u64) -> Option<(String, u64)> {
        if self.is_paired(user_id) {
            return None;
        }
        let mut pending = self.pending.lock().unwrap();
        if let Some(p) = pending.get(&user_id) {
            return Some((p.code.clone(), p.expires_at));
        }
        let code = generate_pairing_code();
        let expires_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + 300; // 5 minutes
        pending.insert(
            user_id,
            PendingPairing { code: code.clone(), channel_id, expires_at },
        );
        Some((code, expires_at))
    }

    /// Approve by code. Returns `(user_id, channel_id)` if the code was valid.
    pub fn approve_by_code(&self, code: &str) -> Option<(u64, u64)> {
        let mut pending = self.pending.lock().unwrap();
        let user_id = pending
            .iter()
            .find(|(_, p)| p.code == code)
            .map(|(uid, _)| *uid)?;
        let entry = pending.remove(&user_id)?;
        self.approved.lock().unwrap().insert(user_id);
        Some((user_id, entry.channel_id))
    }

    /// Reject by code. Returns `(user_id, channel_id)` if the code was valid.
    pub fn reject_by_code(&self, code: &str) -> Option<(u64, u64)> {
        let mut pending = self.pending.lock().unwrap();
        let user_id = pending
            .iter()
            .find(|(_, p)| p.code == code)
            .map(|(uid, _)| *uid)?;
        let entry = pending.remove(&user_id)?;
        Some((user_id, entry.channel_id))
    }
}

/// Generate a 6-character alphanumeric pairing code (no rand crate needed).
fn generate_pairing_code() -> String {
    // Use the lower bits of nanosecond timestamp as entropy source.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    // Mix with memory address of a local variable for extra variance.
    let addr = &nanos as *const u32 as u64;
    let mut n = (nanos as u64).wrapping_add(addr.wrapping_mul(6364136223846793005));
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789"; // 32 chars
    let mut code = String::with_capacity(6);
    for _ in 0..6 {
        code.push(ALPHABET[(n % 32) as usize] as char);
        n >>= 5;
    }
    code
}
use serenity::model::application::{
    CommandInteraction, ComponentInteraction, ComponentInteractionDataKind,
    ModalInteraction, ResolvedValue,
};
use serenity::model::channel::GuildChannel;
use serenity::model::event::ChannelPinsUpdateEvent as SerenityChannelPinsUpdateEvent;
use serenity::gateway::ConnectionStage;
use serenity::gateway::ShardStageUpdateEvent as SerenityShardStageUpdateEvent;
use serenity::model::application::CommandPermissions as SerenityCommandPermissions;
use serenity::model::event::GuildMembersChunkEvent as SerenityGuildMembersChunkEvent;
use serenity::model::event::SoundboardSoundCreateEvent as SerenitySoundboardSoundCreateEvent;
use serenity::model::event::SoundboardSoundDeleteEvent as SerenitySoundboardSoundDeleteEvent;
use serenity::model::event::SoundboardSoundUpdateEvent as SerenitySoundboardSoundUpdateEvent;
use serenity::model::event::SoundboardSoundsEvent as SerenitySoundboardSoundsEvent;
use serenity::model::event::SoundboardSoundsUpdateEvent as SerenitySoundboardSoundsUpdateEvent;
use serenity::model::soundboard::Soundboard as SerenitySoundboard;
use serenity::model::event::GuildScheduledEventUserAddEvent as SerenityScheduledEventUserAddEvent;
use serenity::model::event::GuildScheduledEventUserRemoveEvent as SerenityScheduledEventUserRemoveEvent;
use serenity::model::event::MessagePollVoteAddEvent as SerenityPollVoteAddEvent;
use serenity::model::event::MessagePollVoteRemoveEvent as SerenityPollVoteRemoveEvent;
use serenity::model::event::ThreadListSyncEvent as SerenityThreadListSyncEvent;
use serenity::model::event::TypingStartEvent as SerenityTypingStartEvent;
use serenity::model::event::VoiceServerUpdateEvent as SerenityVoiceServerUpdateEvent;
use serenity::model::guild::automod::{ActionExecution as SerenityActionExecution, Rule as SerenityAutoModRule};
use serenity::model::guild::Integration as SerenityIntegration;
use serenity::model::id::{ApplicationId, IntegrationId};
use serenity::model::monetization::Entitlement as SerenityEntitlement;
use serenity::model::user::CurrentUser;
use serenity::model::guild::ScheduledEvent;
use serenity::model::gateway::{Presence, Ready};
use serenity::model::guild::{PartialGuild, Role, UnavailableGuild};
use serenity::model::voice::VoiceState as SerenityVoiceState;
use serenity::model::channel::Message as SerenityMessage;
use serenity::model::channel::ReactionType;
use serenity::model::event::MessageUpdateEvent;
use serenity::model::guild::Member;
use serenity::model::id::{ChannelId, GuildId, MessageId};
use serenity::model::user::User as SerenityUser;
use serenity::prelude::TypeMapKey;
use tracing::{debug, warn};

/// Discord → NATS bridge
pub struct DiscordBridge {
    publisher: MessagePublisher,
    pub access_config: AccessConfig,
    sequence: Arc<AtomicU64>,
    bot_user_id: Arc<AtomicU64>,
    /// Whether to publish presence update events (requires GUILD_PRESENCES intent)
    pub presence_enabled: bool,
    /// When set, slash commands are registered to this guild ID instead of globally.
    pub guild_commands_guild_id: Option<u64>,
    /// Runtime state for the DM pairing flow (shared with OutboundProcessor).
    pub pairing_state: Arc<PairingState>,
    /// Controls which reaction events are forwarded to NATS.
    pub reaction_mode: crate::config::ReactionMode,
    /// User IDs allowed to trigger reaction events when reaction_mode = allowlist.
    pub reaction_allowlist: Vec<u64>,
    /// Optional emoji to react with when a message is received and will be processed.
    pub ack_reaction: Option<String>,
    /// Whether to process messages sent by other bots.
    pub allow_bots: bool,
}

impl TypeMapKey for DiscordBridge {
    type Value = Arc<DiscordBridge>;
}

impl DiscordBridge {
    /// Create a new bridge
    pub fn new(
        client: async_nats::Client,
        prefix: String,
        access_config: AccessConfig,
        presence_enabled: bool,
        guild_commands_guild_id: Option<u64>,
        reaction_mode: crate::config::ReactionMode,
        reaction_allowlist: Vec<u64>,
        ack_reaction: Option<String>,
        allow_bots: bool,
    ) -> Self {
        Self {
            publisher: MessagePublisher::new(client, prefix),
            access_config,
            sequence: Arc::new(AtomicU64::new(0)),
            bot_user_id: Arc::new(AtomicU64::new(0)),
            presence_enabled,
            guild_commands_guild_id,
            pairing_state: Arc::new(PairingState::new()),
            reaction_mode,
            reaction_allowlist,
            ack_reaction,
            allow_bots,
        }
    }

    /// Returns the bot's own user ID (0 if not yet set).
    pub fn bot_user_id(&self) -> u64 {
        self.bot_user_id.load(Ordering::Relaxed)
    }

    /// Returns true if reaction events should be forwarded based on the configured mode.
    ///
    /// `is_bot_message` — whether the message was sent by this bot.
    /// `reactor_user_id` — the user who added/removed the reaction.
    pub fn should_publish_reaction(&self, is_bot_message: bool, reactor_user_id: Option<u64>) -> bool {
        match self.reaction_mode {
            crate::config::ReactionMode::Off => false,
            crate::config::ReactionMode::Own => is_bot_message,
            crate::config::ReactionMode::All => true,
            crate::config::ReactionMode::Allowlist => reactor_user_id
                .map(|id| self.reaction_allowlist.contains(&id))
                .unwrap_or(false),
        }
    }

    /// Store the bot's own user ID (called from the ready handler)
    pub fn set_bot_user_id(&self, id: u64) {
        self.bot_user_id.store(id, Ordering::Relaxed);
    }

    /// Returns true if the message should be processed.
    ///
    /// When `require_mention` is false every message passes.
    /// When `require_mention` is true the bot's own user ID must appear in
    /// the mentions list.  If the bot ID is not set yet (0) the check is
    /// skipped to avoid silently dropping messages on startup.
    pub fn check_require_mention(&self, mentions: &[serenity::model::user::User]) -> bool {
        if !self.access_config.require_mention {
            return true;
        }
        let bot_id = self.bot_user_id.load(Ordering::Relaxed);
        if bot_id == 0 {
            return true;
        }
        mentions.iter().any(|u| u.id.get() == bot_id)
    }

    fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed)
    }

    fn prefix(&self) -> &str {
        self.publisher.prefix()
    }

    /// Check if a user has guild access
    pub fn check_guild_access(&self, guild_id: u64) -> bool {
        self.access_config.can_access_guild(guild_id)
    }

    /// Check if a user has DM access.
    ///
    /// For `DmPolicy::Pairing` the static config always returns `false`; the
    /// runtime pairing_state is checked here to allow previously-approved users.
    pub fn check_dm_access(&self, user_id: u64) -> bool {
        if self.access_config.is_admin(user_id) {
            return true;
        }
        if self.access_config.dm_policy == DmPolicy::Pairing {
            return self.pairing_state.is_paired(user_id);
        }
        self.access_config.can_access_dm(user_id)
    }

    /// Attempt to start a pairing session for a user who sent a DM.
    ///
    /// Returns `Some((code, expires_at))` when a new or existing pending
    /// session is available; `None` when the user is already approved.
    pub fn try_start_pairing(&self, user_id: u64, channel_id: u64) -> Option<(String, u64)> {
        self.pairing_state.start_pairing(user_id, channel_id)
    }

    /// Publish a pairing_requested event so admins are notified.
    pub async fn publish_pairing_requested(
        &self,
        user_id: u64,
        username: &str,
        code: &str,
        expires_at: u64,
    ) -> Result<()> {
        let sid = session_id(&ChannelType::Dm, user_id, None, Some(user_id));
        let meta = EventMetadata::new(sid, self.next_sequence());
        let event = PairingRequestedEvent {
            metadata: meta,
            user_id,
            username: username.to_string(),
            code: code.to_string(),
            expires_at,
        };
        self.publisher
            .publish(&subjects::bot::pairing_requested(self.prefix()), &event)
            .await?;
        Ok(())
    }

    /// Check if a channel is allowed
    pub fn check_channel_access(&self, channel_id: u64) -> bool {
        self.access_config.can_access_channel(channel_id)
    }

    /// Check if a user is an admin
    pub fn is_admin(&self, user_id: u64) -> bool {
        self.access_config.is_admin(user_id)
    }

    // ── Conversion helpers ─────────────────────────────────────────────────

    pub fn convert_user(user: &SerenityUser) -> DiscordUser {
        DiscordUser {
            id: user.id.get(),
            username: user.name.clone(),
            global_name: user.global_name.as_deref().map(String::from),
            bot: user.bot,
        }
    }

    pub fn convert_message(msg: &SerenityMessage) -> DiscordMessage {
        let attachments = msg
            .attachments
            .iter()
            .map(|a| Attachment {
                id: a.id.get(),
                filename: a.filename.clone(),
                url: a.url.clone(),
                content_type: a.content_type.clone(),
                size: a.size as u64,
            })
            .collect();

        let embeds = msg.embeds.iter().map(Self::convert_embed).collect();

        DiscordMessage {
            id: msg.id.get(),
            channel_id: msg.channel_id.get(),
            guild_id: msg.guild_id.map(|g| g.get()),
            author: Self::convert_user(&msg.author),
            content: msg.content.clone(),
            timestamp: msg.timestamp.to_rfc3339().unwrap_or_default(),
            edited_timestamp: msg.edited_timestamp.and_then(|t| t.to_rfc3339()),
            attachments,
            embeds,
            referenced_message_id: msg.referenced_message.as_ref().map(|r| r.id.get()),
            referenced_message_content: msg
                .referenced_message
                .as_ref()
                .map(|r| r.content.clone())
                .filter(|c| !c.is_empty()),
        }
    }

    fn convert_embed(e: &serenity::model::channel::Embed) -> Embed {
        Embed {
            title: e.title.clone(),
            description: e.description.clone(),
            url: e.url.clone(),
            color: e.colour.map(|c| c.0),
            fields: e
                .fields
                .iter()
                .map(|f| EmbedField {
                    name: f.name.clone(),
                    value: f.value.clone(),
                    inline: f.inline,
                })
                .collect(),
            author: e.author.as_ref().map(|a| EmbedAuthor {
                name: a.name.clone(),
                url: a.url.clone(),
                icon_url: a.icon_url.clone(),
            }),
            footer: e.footer.as_ref().map(|f| EmbedFooter {
                text: f.text.clone(),
                icon_url: f.icon_url.clone(),
            }),
            image: e.image.as_ref().map(|i| EmbedMedia { url: i.url.clone() }),
            thumbnail: e.thumbnail.as_ref().map(|t| EmbedMedia { url: t.url.clone() }),
            timestamp: e.timestamp.map(|ts| ts.to_string()),
        }
    }

    fn session_id_for_message(msg: &SerenityMessage) -> String {
        if msg.guild_id.is_some() {
            session_id(
                &ChannelType::GuildText,
                msg.channel_id.get(),
                msg.guild_id.map(|g| g.get()),
                Some(msg.author.id.get()),
            )
        } else {
            session_id(
                &ChannelType::Dm,
                msg.channel_id.get(),
                None,
                Some(msg.author.id.get()),
            )
        }
    }

    // ── Publish methods ────────────────────────────────────────────────────

    pub async fn publish_message_created(&self, msg: &SerenityMessage) -> Result<()> {
        let sid = Self::session_id_for_message(msg);
        let meta = EventMetadata::new(sid, self.next_sequence());
        let event = MessageCreatedEvent {
            metadata: meta,
            message: Self::convert_message(msg),
        };
        let subject = subjects::bot::message_created(self.prefix());
        self.publisher.publish(&subject, &event).await?;
        debug!("Published message_created to {}", subject);
        Ok(())
    }

    pub async fn publish_message_updated(&self, event_data: &MessageUpdateEvent) -> Result<()> {
        let guild_id = event_data.guild_id.map(|g| g.get());
        let channel_id = event_data.channel_id.get();
        let chan_type = if guild_id.is_some() {
            ChannelType::GuildText
        } else {
            ChannelType::Dm
        };
        let sid = session_id(&chan_type, channel_id, guild_id, None);
        let meta = EventMetadata::new(sid, self.next_sequence());

        let event = MessageUpdatedEvent {
            metadata: meta,
            message_id: event_data.id.get(),
            channel_id,
            guild_id,
            new_content: event_data.content.clone(),
            new_embeds: event_data
                .embeds
                .as_ref()
                .map(|embeds| embeds.iter().map(Self::convert_embed).collect())
                .unwrap_or_default(),
        };

        let subject = subjects::bot::message_updated(self.prefix());
        self.publisher.publish(&subject, &event).await?;
        debug!("Published message_updated to {}", subject);
        Ok(())
    }

    pub async fn publish_message_deleted(
        &self,
        channel_id: ChannelId,
        guild_id: Option<GuildId>,
        message_id: MessageId,
    ) -> Result<()> {
        let guild_id_u64 = guild_id.map(|g| g.get());
        let channel_id_u64 = channel_id.get();
        let chan_type = if guild_id_u64.is_some() {
            ChannelType::GuildText
        } else {
            ChannelType::Dm
        };
        let sid = session_id(&chan_type, channel_id_u64, guild_id_u64, None);
        let meta = EventMetadata::new(sid, self.next_sequence());

        let event = MessageDeletedEvent {
            metadata: meta,
            message_id: message_id.get(),
            channel_id: channel_id_u64,
            guild_id: guild_id_u64,
        };

        let subject = subjects::bot::message_deleted(self.prefix());
        self.publisher.publish(&subject, &event).await?;
        debug!("Published message_deleted to {}", subject);
        Ok(())
    }

    pub async fn publish_slash_command(&self, interaction: &CommandInteraction) -> Result<()> {
        let guild_id = interaction.guild_id.map(|g| g.get());
        let channel_id = interaction.channel_id.get();
        let chan_type = if guild_id.is_some() {
            ChannelType::GuildText
        } else {
            ChannelType::Dm
        };
        let sid = session_id(
            &chan_type,
            channel_id,
            guild_id,
            Some(interaction.user.id.get()),
        );
        let meta = EventMetadata::new(sid, self.next_sequence());

        let options: Vec<CommandOption> = interaction
            .data
            .options()
            .iter()
            .filter_map(|opt| {
                let value = match &opt.value {
                    ResolvedValue::Boolean(b) => CommandOptionValue::Boolean(*b),
                    ResolvedValue::Integer(i) => CommandOptionValue::Integer(*i),
                    ResolvedValue::Number(n) => CommandOptionValue::Number(*n),
                    ResolvedValue::String(s) => CommandOptionValue::String(s.to_string()),
                    ResolvedValue::User(u, _) => CommandOptionValue::User(u.id.get()),
                    ResolvedValue::Channel(c) => CommandOptionValue::Channel(c.id.get()),
                    ResolvedValue::Role(r) => CommandOptionValue::Role(r.id.get()),
                    ResolvedValue::Attachment(a) => CommandOptionValue::String(a.url.clone()),
                    ResolvedValue::SubCommand(_) | ResolvedValue::SubCommandGroup(_) => {
                        // Sub-commands are structural, not values — skip
                        return None;
                    }
                    ResolvedValue::Autocomplete { value, .. } => {
                        CommandOptionValue::String(value.to_string())
                    }
                    ResolvedValue::Unresolved(_) => {
                        // Could not be resolved (e.g. missing from resolved map) — skip
                        return None;
                    }
                    _ => {
                        // Future serenity variants: preserve as empty string rather than panicking
                        CommandOptionValue::String(String::new())
                    }
                };
                Some(CommandOption {
                    name: opt.name.to_string(),
                    value,
                })
            })
            .collect();

        let event = SlashCommandEvent {
            metadata: meta,
            interaction_id: interaction.id.get(),
            interaction_token: interaction.token.clone(),
            guild_id,
            channel_id,
            user: Self::convert_user(&interaction.user),
            command_name: interaction.data.name.clone(),
            options,
        };

        let subject = subjects::bot::interaction_command(self.prefix());
        self.publisher.publish(&subject, &event).await?;
        debug!("Published interaction_command to {}", subject);
        Ok(())
    }

    pub async fn publish_component_interaction(
        &self,
        interaction: &ComponentInteraction,
    ) -> Result<()> {
        let guild_id = interaction.guild_id.map(|g| g.get());
        let channel_id = interaction.channel_id.get();
        let chan_type = if guild_id.is_some() {
            ChannelType::GuildText
        } else {
            ChannelType::Dm
        };
        let sid = session_id(
            &chan_type,
            channel_id,
            guild_id,
            Some(interaction.user.id.get()),
        );
        let meta = EventMetadata::new(sid, self.next_sequence());

        let (component_type, values) = match interaction.data.kind {
            ComponentInteractionDataKind::Button => (ComponentType::Button, vec![]),
            ComponentInteractionDataKind::StringSelect { ref values } => (
                ComponentType::StringSelect,
                values.clone(),
            ),
            ComponentInteractionDataKind::UserSelect { ref values } => (
                ComponentType::UserSelect,
                values.iter().map(|id| id.get().to_string()).collect(),
            ),
            ComponentInteractionDataKind::RoleSelect { ref values } => (
                ComponentType::RoleSelect,
                values.iter().map(|id| id.get().to_string()).collect(),
            ),
            ComponentInteractionDataKind::MentionableSelect { ref values } => (
                ComponentType::MentionableSelect,
                values.iter().map(|id| id.get().to_string()).collect(),
            ),
            ComponentInteractionDataKind::ChannelSelect { ref values } => (
                ComponentType::ChannelSelect,
                values.iter().map(|id| id.get().to_string()).collect(),
            ),
            _ => (ComponentType::Unknown, vec![]),
        };

        let event = ComponentInteractionEvent {
            metadata: meta,
            interaction_id: interaction.id.get(),
            interaction_token: interaction.token.clone(),
            guild_id,
            channel_id,
            user: Self::convert_user(&interaction.user),
            message_id: interaction.message.id.get(),
            custom_id: interaction.data.custom_id.clone(),
            component_type,
            values,
        };

        let subject = subjects::bot::interaction_component(self.prefix());
        self.publisher.publish(&subject, &event).await?;
        debug!("Published interaction_component to {}", subject);
        Ok(())
    }

    pub async fn publish_reaction_add(
        &self,
        reaction: &serenity::model::channel::Reaction,
    ) -> Result<()> {
        let Some(user_id) = reaction.user_id else {
            warn!("Reaction without user_id, skipping");
            return Ok(());
        };

        let guild_id = reaction.guild_id.map(|g| g.get());
        let channel_id = reaction.channel_id.get();
        let chan_type = if guild_id.is_some() {
            ChannelType::GuildText
        } else {
            ChannelType::Dm
        };
        let sid = session_id(&chan_type, channel_id, guild_id, Some(user_id.get()));
        let meta = EventMetadata::new(sid, self.next_sequence());

        let event = ReactionAddEvent {
            metadata: meta,
            user_id: user_id.get(),
            channel_id,
            message_id: reaction.message_id.get(),
            guild_id,
            emoji: convert_reaction_type(&reaction.emoji),
        };

        let subject = subjects::bot::reaction_add(self.prefix());
        self.publisher.publish(&subject, &event).await?;
        debug!("Published reaction_add to {}", subject);
        Ok(())
    }

    pub async fn publish_reaction_remove(
        &self,
        reaction: &serenity::model::channel::Reaction,
    ) -> Result<()> {
        let Some(user_id) = reaction.user_id else {
            warn!("Reaction without user_id, skipping");
            return Ok(());
        };

        let guild_id = reaction.guild_id.map(|g| g.get());
        let channel_id = reaction.channel_id.get();
        let chan_type = if guild_id.is_some() {
            ChannelType::GuildText
        } else {
            ChannelType::Dm
        };
        let sid = session_id(&chan_type, channel_id, guild_id, Some(user_id.get()));
        let meta = EventMetadata::new(sid, self.next_sequence());

        let event = ReactionRemoveEvent {
            metadata: meta,
            user_id: user_id.get(),
            channel_id,
            message_id: reaction.message_id.get(),
            guild_id,
            emoji: convert_reaction_type(&reaction.emoji),
        };

        let subject = subjects::bot::reaction_remove(self.prefix());
        self.publisher.publish(&subject, &event).await?;
        debug!("Published reaction_remove to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_member_add(&self, guild_id: u64, member: &Member) -> Result<()> {
        let sid = member_session_id(guild_id);
        let meta = EventMetadata::new(sid, self.next_sequence());

        let discord_member = DiscordMember {
            user: Self::convert_user(&member.user),
            guild_id,
            nick: member.nick.clone(),
            roles: member.roles.iter().map(|r| r.get()).collect(),
        };

        let event = GuildMemberAddEvent {
            metadata: meta,
            guild_id,
            member: discord_member,
        };

        let subject = subjects::bot::guild_member_add(self.prefix());
        self.publisher.publish(&subject, &event).await?;
        debug!("Published guild_member_add to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_member_remove(
        &self,
        guild_id: u64,
        user: &SerenityUser,
    ) -> Result<()> {
        let sid = member_session_id(guild_id);
        let meta = EventMetadata::new(sid, self.next_sequence());

        let event = GuildMemberRemoveEvent {
            metadata: meta,
            guild_id,
            user: Self::convert_user(user),
        };

        let subject = subjects::bot::guild_member_remove(self.prefix());
        self.publisher.publish(&subject, &event).await?;
        debug!("Published guild_member_remove to {}", subject);
        Ok(())
    }

    // ── Conversion helpers (new event types) ───────────────────────────────

    pub fn convert_channel(ch: &GuildChannel) -> DiscordChannel {
        use serenity::model::channel::ChannelType as SerenityChannelType;
        DiscordChannel {
            id: ch.id.get(),
            channel_type: match ch.kind {
                SerenityChannelType::Text => ChannelType::GuildText,
                SerenityChannelType::Voice => ChannelType::GuildVoice,
                SerenityChannelType::Category => ChannelType::GuildCategory,
                SerenityChannelType::News => ChannelType::GuildNews,
                SerenityChannelType::Stage => ChannelType::GuildStageVoice,
                SerenityChannelType::Forum => ChannelType::GuildForum,
                _ => ChannelType::Unknown,
            },
            guild_id: Some(ch.guild_id.get()),
            name: Some(ch.name.clone()),
        }
    }

    pub fn convert_role(role: &Role) -> DiscordRole {
        DiscordRole {
            id: role.id.get(),
            name: role.name.clone(),
            color: role.colour.0,
            hoist: role.hoist,
            position: role.position as i64,
            permissions: role.permissions.bits().to_string(),
            mentionable: role.mentionable,
        }
    }

    pub fn convert_voice_state(vs: &SerenityVoiceState) -> BridgeVoiceState {
        BridgeVoiceState {
            user_id: vs.user_id.get(),
            channel_id: vs.channel_id.map(|c| c.get()),
            guild_id: vs.guild_id.map(|g| g.get()),
            self_mute: vs.self_mute,
            self_deaf: vs.self_deaf,
        }
    }

    // ── New publish methods ────────────────────────────────────────────────

    pub async fn publish_typing_start(&self, event: &SerenityTypingStartEvent) -> Result<()> {
        let guild_id = event.guild_id.map(|g| g.get());
        let sid = if let Some(gid) = guild_id {
            member_session_id(gid)
        } else {
            format!("dc-dm-{}", event.user_id.get())
        };
        let meta = EventMetadata::new(sid, self.next_sequence());

        let ev = TypingStartEvent {
            metadata: meta,
            user_id: event.user_id.get(),
            channel_id: event.channel_id.get(),
            guild_id,
        };

        let subject = subjects::bot::typing_start(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published typing_start to {}", subject);
        Ok(())
    }

    pub async fn publish_voice_state_update(
        &self,
        old: Option<&SerenityVoiceState>,
        new: &SerenityVoiceState,
    ) -> Result<()> {
        let guild_id = new.guild_id.map(|g| g.get());
        let sid = if let Some(gid) = guild_id {
            member_session_id(gid)
        } else {
            format!("dc-dm-{}", new.user_id.get())
        };
        let meta = EventMetadata::new(sid, self.next_sequence());

        let ev = VoiceStateUpdateEvent {
            metadata: meta,
            guild_id,
            old_channel_id: old.and_then(|vs| vs.channel_id).map(|c| c.get()),
            new_state: Self::convert_voice_state(new),
        };

        let subject = subjects::bot::voice_state_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published voice_state_update to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_create(
        &self,
        guild: &serenity::model::guild::Guild,
        member_count: u64,
    ) -> Result<()> {
        let sid = member_session_id(guild.id.get());
        let meta = EventMetadata::new(sid, self.next_sequence());

        let ev = GuildCreateEvent {
            metadata: meta,
            guild: DiscordGuild {
                id: guild.id.get(),
                name: guild.name.clone(),
            },
            member_count,
        };

        let subject = subjects::bot::guild_create(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published guild_create to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_update(&self, guild: &PartialGuild) -> Result<()> {
        let sid = member_session_id(guild.id.get());
        let meta = EventMetadata::new(sid, self.next_sequence());

        let ev = GuildUpdateEvent {
            metadata: meta,
            guild: DiscordGuild {
                id: guild.id.get(),
                name: guild.name.clone(),
            },
        };

        let subject = subjects::bot::guild_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published guild_update to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_delete(&self, incomplete: &UnavailableGuild) -> Result<()> {
        let sid = member_session_id(incomplete.id.get());
        let meta = EventMetadata::new(sid, self.next_sequence());

        let ev = GuildDeleteEvent {
            metadata: meta,
            guild_id: incomplete.id.get(),
            unavailable: incomplete.unavailable,
        };

        let subject = subjects::bot::guild_delete(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published guild_delete to {}", subject);
        Ok(())
    }

    pub async fn publish_channel_create(&self, channel: &GuildChannel) -> Result<()> {
        let sid = member_session_id(channel.guild_id.get());
        let meta = EventMetadata::new(sid, self.next_sequence());

        let ev = ChannelCreateEvent {
            metadata: meta,
            channel: Self::convert_channel(channel),
        };

        let subject = subjects::bot::channel_create(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published channel_create to {}", subject);
        Ok(())
    }

    pub async fn publish_channel_update(&self, channel: &GuildChannel) -> Result<()> {
        let sid = member_session_id(channel.guild_id.get());
        let meta = EventMetadata::new(sid, self.next_sequence());

        let ev = ChannelUpdateEvent {
            metadata: meta,
            channel: Self::convert_channel(channel),
        };

        let subject = subjects::bot::channel_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published channel_update to {}", subject);
        Ok(())
    }

    pub async fn publish_channel_delete(
        &self,
        channel_id: u64,
        guild_id: u64,
    ) -> Result<()> {
        let sid = member_session_id(guild_id);
        let meta = EventMetadata::new(sid, self.next_sequence());

        let ev = ChannelDeleteEvent {
            metadata: meta,
            channel_id,
            guild_id,
        };

        let subject = subjects::bot::channel_delete(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published channel_delete to {}", subject);
        Ok(())
    }

    pub async fn publish_role_create(&self, guild_id: u64, role: &Role) -> Result<()> {
        let sid = member_session_id(guild_id);
        let meta = EventMetadata::new(sid, self.next_sequence());

        let ev = RoleCreateEvent {
            metadata: meta,
            guild_id,
            role: Self::convert_role(role),
        };

        let subject = subjects::bot::role_create(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published role_create to {}", subject);
        Ok(())
    }

    pub async fn publish_role_update(&self, guild_id: u64, role: &Role) -> Result<()> {
        let sid = member_session_id(guild_id);
        let meta = EventMetadata::new(sid, self.next_sequence());

        let ev = RoleUpdateEvent {
            metadata: meta,
            guild_id,
            role: Self::convert_role(role),
        };

        let subject = subjects::bot::role_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published role_update to {}", subject);
        Ok(())
    }

    pub async fn publish_role_delete(
        &self,
        guild_id: u64,
        role_id: u64,
    ) -> Result<()> {
        let sid = member_session_id(guild_id);
        let meta = EventMetadata::new(sid, self.next_sequence());

        let ev = RoleDeleteEvent {
            metadata: meta,
            guild_id,
            role_id,
        };

        let subject = subjects::bot::role_delete(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published role_delete to {}", subject);
        Ok(())
    }

    pub async fn publish_presence_update(&self, presence: &Presence) -> Result<()> {
        let Some(guild_id) = presence.guild_id else {
            return Ok(());
        };
        let sid = member_session_id(guild_id.get());
        let meta = EventMetadata::new(sid, self.next_sequence());

        use serenity::model::user::OnlineStatus;
        let status = match presence.status {
            OnlineStatus::Online => "online",
            OnlineStatus::Idle => "idle",
            OnlineStatus::DoNotDisturb => "dnd",
            OnlineStatus::Offline | OnlineStatus::Invisible => "offline",
            _ => "unknown",
        }
        .to_string();

        let ev = PresenceUpdateEvent {
            metadata: meta,
            user_id: presence.user.id.get(),
            guild_id: guild_id.get(),
            status,
        };

        let subject = subjects::bot::presence_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published presence_update to {}", subject);
        Ok(())
    }

    pub async fn publish_modal_submit(&self, interaction: &ModalInteraction) -> Result<()> {
        let guild_id = interaction.guild_id.map(|g| g.get());
        let channel_id = interaction.channel_id.get();
        let sid = if let Some(gid) = guild_id {
            session_id(
                &ChannelType::GuildText,
                channel_id,
                Some(gid),
                Some(interaction.user.id.get()),
            )
        } else {
            session_id(
                &ChannelType::Dm,
                channel_id,
                None,
                Some(interaction.user.id.get()),
            )
        };
        let meta = EventMetadata::new(sid, self.next_sequence());

        use serenity::model::application::ActionRowComponent;
        let inputs: Vec<ModalInput> = interaction
            .data
            .components
            .iter()
            .flat_map(|row| row.components.iter())
            .filter_map(|comp| {
                if let ActionRowComponent::InputText(input) = comp {
                    Some(ModalInput {
                        custom_id: input.custom_id.clone(),
                        value: input.value.clone().unwrap_or_default(),
                    })
                } else {
                    None
                }
            })
            .collect();

        let ev = ModalSubmitEvent {
            metadata: meta,
            interaction_id: interaction.id.get(),
            interaction_token: interaction.token.clone(),
            guild_id,
            channel_id,
            user: Self::convert_user(&interaction.user),
            custom_id: interaction.data.custom_id.clone(),
            inputs,
        };

        let subject = subjects::bot::interaction_modal(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published interaction_modal to {}", subject);
        Ok(())
    }

    pub async fn publish_autocomplete(&self, interaction: &CommandInteraction) -> Result<()> {
        let guild_id = interaction.guild_id.map(|g| g.get());
        let channel_id = interaction.channel_id.get();
        let sid = session_id(
            if guild_id.is_some() {
                &ChannelType::GuildText
            } else {
                &ChannelType::Dm
            },
            channel_id,
            guild_id,
            Some(interaction.user.id.get()),
        );
        let meta = EventMetadata::new(sid, self.next_sequence());

        // Use CommandData::autocomplete() which finds the option with the Autocomplete variant,
        // i.e. the one the user is currently typing into.
        let focused = interaction.data.autocomplete();
        let focused_option = focused.as_ref().map(|o| o.name.to_string()).unwrap_or_default();
        let current_value = focused.as_ref().map(|o| o.value.to_string()).unwrap_or_default();

        let ev = AutocompleteEvent {
            metadata: meta,
            interaction_id: interaction.id.get(),
            interaction_token: interaction.token.clone(),
            guild_id,
            channel_id,
            user: Self::convert_user(&interaction.user),
            command_name: interaction.data.name.clone(),
            focused_option,
            current_value,
        };

        let subject = subjects::bot::interaction_autocomplete(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published interaction_autocomplete to {}", subject);
        Ok(())
    }

    pub async fn publish_message_bulk_deleted(
        &self,
        channel_id: ChannelId,
        guild_id: Option<GuildId>,
        message_ids: Vec<MessageId>,
    ) -> Result<()> {
        let sid = if let Some(gid) = guild_id {
            member_session_id(gid.get())
        } else {
            format!("dc-dm-{}", channel_id.get())
        };
        let meta = EventMetadata::new(sid, self.next_sequence());

        let event = MessageBulkDeleteEvent {
            metadata: meta,
            channel_id: channel_id.get(),
            guild_id: guild_id.map(|g| g.get()),
            message_ids: message_ids.iter().map(|m| m.get()).collect(),
        };

        let subject = subjects::bot::message_bulk_delete(self.prefix());
        self.publisher.publish(&subject, &event).await?;
        debug!("Published message_bulk_delete to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_member_updated(
        &self,
        guild_id: GuildId,
        user: &SerenityUser,
        nick: Option<String>,
        roles: Vec<u64>,
    ) -> Result<()> {
        let sid = member_session_id(guild_id.get());
        let meta = EventMetadata::new(sid, self.next_sequence());

        let event = GuildMemberUpdateEvent {
            metadata: meta,
            guild_id: guild_id.get(),
            user: Self::convert_user(user),
            nick,
            roles,
        };

        let subject = subjects::bot::guild_member_update(self.prefix());
        self.publisher.publish(&subject, &event).await?;
        debug!("Published guild_member_update to {}", subject);
        Ok(())
    }

    pub async fn publish_bot_ready(&self, ready: &Ready) -> Result<()> {
        let sid = format!("dc-bot-{}", ready.user.id.get());
        let meta = EventMetadata::new(sid, self.next_sequence());

        let ev = BotReadyEvent {
            metadata: meta,
            bot_user: Self::convert_user(&ready.user),
            guild_count: ready.guilds.len() as u64,
        };

        let subject = subjects::bot::bot_ready(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published bot_ready to {}", subject);
        Ok(())
    }

    pub async fn publish_invite_create(
        &self,
        data: &serenity::model::event::InviteCreateEvent,
    ) -> Result<()> {
        let guild_id = data.guild_id.map(|g| g.get());
        let meta = EventMetadata::new(
            format!("dc-guild-{}", guild_id.unwrap_or(0)),
            self.next_sequence(),
        );
        let ev = InviteCreateEvent {
            metadata: meta,
            code: data.code.clone(),
            channel_id: data.channel_id.get(),
            guild_id,
            inviter_id: data.inviter.as_ref().map(|u| u.id.get()),
            max_age_secs: data.max_age as u64,
            max_uses: data.max_uses as u64,
            temporary: data.temporary,
        };
        let subject = subjects::bot::invite_create(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published invite_create to {}", subject);
        Ok(())
    }

    pub async fn publish_invite_delete(
        &self,
        data: &serenity::model::event::InviteDeleteEvent,
    ) -> Result<()> {
        let guild_id = data.guild_id.map(|g| g.get());
        let meta = EventMetadata::new(
            format!("dc-guild-{}", guild_id.unwrap_or(0)),
            self.next_sequence(),
        );
        let ev = InviteDeleteEvent {
            metadata: meta,
            code: data.code.clone(),
            channel_id: data.channel_id.get(),
            guild_id,
        };
        let subject = subjects::bot::invite_delete(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published invite_delete to {}", subject);
        Ok(())
    }

    pub async fn publish_thread_created(&self, thread: &GuildChannel) -> Result<()> {
        let meta = EventMetadata::new(
            format!("dc-guild-{}", thread.guild_id.get()),
            self.next_sequence(),
        );
        let tm = thread.thread_metadata.as_ref();
        let ev = ThreadCreatedEvent {
            metadata: meta,
            thread_id: thread.id.get(),
            guild_id: thread.guild_id.get(),
            parent_id: thread.parent_id.map(|id| id.get()).unwrap_or(0),
            name: thread.name.clone(),
            archived: tm.map(|m| m.archived).unwrap_or(false),
            locked: tm.map(|m| m.locked).unwrap_or(false),
            auto_archive_duration: tm
                .map(|m| u16::from(m.auto_archive_duration) as u64)
                .unwrap_or(0),
        };
        let subject = subjects::bot::thread_created(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published thread_created to {}", subject);
        Ok(())
    }

    pub async fn publish_thread_updated(&self, thread: &GuildChannel) -> Result<()> {
        let meta = EventMetadata::new(
            format!("dc-guild-{}", thread.guild_id.get()),
            self.next_sequence(),
        );
        let tm = thread.thread_metadata.as_ref();
        let ev = ThreadUpdatedEvent {
            metadata: meta,
            thread_id: thread.id.get(),
            guild_id: thread.guild_id.get(),
            parent_id: thread.parent_id.map(|id| id.get()).unwrap_or(0),
            name: thread.name.clone(),
            archived: tm.map(|m| m.archived).unwrap_or(false),
            locked: tm.map(|m| m.locked).unwrap_or(false),
        };
        let subject = subjects::bot::thread_updated(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published thread_updated to {}", subject);
        Ok(())
    }

    pub async fn publish_thread_deleted(
        &self,
        thread_id: u64,
        guild_id: u64,
        parent_id: Option<u64>,
    ) -> Result<()> {
        let meta = EventMetadata::new(
            format!("dc-guild-{}", guild_id),
            self.next_sequence(),
        );
        let ev = ThreadDeletedEvent {
            metadata: meta,
            thread_id,
            guild_id,
            parent_id,
        };
        let subject = subjects::bot::thread_deleted(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published thread_deleted to {}", subject);
        Ok(())
    }

    pub async fn publish_thread_member_add(
        &self,
        thread_id: u64,
        guild_id: u64,
        user_ids: Vec<u64>,
    ) -> Result<()> {
        let meta = EventMetadata::new(
            format!("dc-guild-{}", guild_id),
            self.next_sequence(),
        );
        let ev = ThreadMemberAddEvent {
            metadata: meta,
            thread_id,
            guild_id,
            user_ids,
        };
        let subject = subjects::bot::thread_member_add(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published thread_member_add to {}", subject);
        Ok(())
    }

    pub async fn publish_thread_member_remove(
        &self,
        thread_id: u64,
        guild_id: u64,
        user_ids: Vec<u64>,
    ) -> Result<()> {
        let meta = EventMetadata::new(
            format!("dc-guild-{}", guild_id),
            self.next_sequence(),
        );
        let ev = ThreadMemberRemoveEvent {
            metadata: meta,
            thread_id,
            guild_id,
            user_ids,
        };
        let subject = subjects::bot::thread_member_remove(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published thread_member_remove to {}", subject);
        Ok(())
    }

    pub async fn publish_stage_instance_create(
        &self,
        stage: &serenity::model::channel::StageInstance,
    ) -> Result<()> {
        let guild_id = stage.guild_id.get();
        let meta = EventMetadata::new(
            format!("dc-guild-{}", guild_id),
            self.next_sequence(),
        );
        let ev = StageInstanceCreateEvent {
            metadata: meta,
            stage_id: stage.id.get(),
            guild_id,
            channel_id: stage.channel_id.get(),
            topic: stage.topic.clone(),
        };
        let subject = subjects::bot::stage_instance_create(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published stage_instance_create to {}", subject);
        Ok(())
    }

    pub async fn publish_stage_instance_update(
        &self,
        stage: &serenity::model::channel::StageInstance,
    ) -> Result<()> {
        let guild_id = stage.guild_id.get();
        let meta = EventMetadata::new(
            format!("dc-guild-{}", guild_id),
            self.next_sequence(),
        );
        let ev = StageInstanceUpdateEvent {
            metadata: meta,
            stage_id: stage.id.get(),
            guild_id,
            channel_id: stage.channel_id.get(),
            topic: stage.topic.clone(),
        };
        let subject = subjects::bot::stage_instance_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published stage_instance_update to {}", subject);
        Ok(())
    }

    pub async fn publish_stage_instance_delete(
        &self,
        stage: &serenity::model::channel::StageInstance,
    ) -> Result<()> {
        let guild_id = stage.guild_id.get();
        let meta = EventMetadata::new(
            format!("dc-guild-{}", guild_id),
            self.next_sequence(),
        );
        let ev = StageInstanceDeleteEvent {
            metadata: meta,
            stage_id: stage.id.get(),
            guild_id,
            channel_id: stage.channel_id.get(),
        };
        let subject = subjects::bot::stage_instance_delete(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published stage_instance_delete to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_ban_add(
        &self,
        guild_id: GuildId,
        user: &serenity::model::user::User,
    ) -> Result<()> {
        let gid = guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let ev = GuildBanAddEvent {
            metadata: meta,
            guild_id: gid,
            user: Self::convert_user(user),
        };
        let subject = subjects::bot::guild_ban_add(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published guild_ban_add to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_ban_remove(
        &self,
        guild_id: GuildId,
        user: &serenity::model::user::User,
    ) -> Result<()> {
        let gid = guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let ev = GuildBanRemoveEvent {
            metadata: meta,
            guild_id: gid,
            user: Self::convert_user(user),
        };
        let subject = subjects::bot::guild_ban_remove(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published guild_ban_remove to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_emojis_update(
        &self,
        guild_id: GuildId,
        current_state: &HashMap<serenity::model::id::EmojiId, serenity::model::guild::Emoji>,
    ) -> Result<()> {
        let gid = guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let emojis: Vec<_> = current_state
            .values()
            .map(|e| Emoji { id: Some(e.id.get()), name: e.name.clone(), animated: e.animated })
            .collect();
        let ev = GuildEmojisUpdateEvent { metadata: meta, guild_id: gid, emojis };
        let subject = subjects::bot::guild_emojis_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published guild_emojis_update to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_scheduled_event_create(&self, event: &ScheduledEvent) -> Result<()> {
        let gid = event.guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let ev = GuildScheduledEventCreateEvent {
            metadata: meta,
            event_id: event.id.get(),
            guild_id: gid,
            name: event.name.clone(),
            description: event.description.clone(),
            start_time: event.start_time.to_rfc3339().unwrap_or_default(),
            channel_id: event.channel_id.map(|c| c.get()),
        };
        let subject = subjects::bot::guild_scheduled_event_create(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published guild_scheduled_event_create to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_scheduled_event_update(&self, event: &ScheduledEvent) -> Result<()> {
        let gid = event.guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let ev = GuildScheduledEventUpdateEvent {
            metadata: meta,
            event_id: event.id.get(),
            guild_id: gid,
            name: event.name.clone(),
            description: event.description.clone(),
            start_time: event.start_time.to_rfc3339().unwrap_or_default(),
            channel_id: event.channel_id.map(|c| c.get()),
        };
        let subject = subjects::bot::guild_scheduled_event_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published guild_scheduled_event_update to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_scheduled_event_delete(&self, event: &ScheduledEvent) -> Result<()> {
        let gid = event.guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let ev = GuildScheduledEventDeleteEvent {
            metadata: meta,
            event_id: event.id.get(),
            guild_id: gid,
            name: event.name.clone(),
        };
        let subject = subjects::bot::guild_scheduled_event_delete(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published guild_scheduled_event_delete to {}", subject);
        Ok(())
    }

    pub async fn publish_channel_pins_update(
        &self,
        pin: &SerenityChannelPinsUpdateEvent,
    ) -> Result<()> {
        let guild_id = pin.guild_id.map(|g| g.get());
        let meta = EventMetadata::new(
            format!("dc-guild-{}", guild_id.unwrap_or(0)),
            self.next_sequence(),
        );
        let ev = ChannelPinsUpdateEvent {
            metadata: meta,
            channel_id: pin.channel_id.get(),
            guild_id,
            last_pin_timestamp: pin.last_pin_timestamp.and_then(|t| t.to_rfc3339()),
        };
        let subject = subjects::bot::channel_pins_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published channel_pins_update to {}", subject);
        Ok(())
    }

    pub async fn publish_reaction_remove_all(
        &self,
        channel_id: ChannelId,
        message_id: MessageId,
    ) -> Result<()> {
        let meta = EventMetadata::new(
            format!("dc-channel-{}", channel_id.get()),
            self.next_sequence(),
        );
        let ev = ReactionRemoveAllEvent {
            metadata: meta,
            channel_id: channel_id.get(),
            message_id: message_id.get(),
        };
        let subject = subjects::bot::reaction_remove_all_notification(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published reaction_remove_all to {}", subject);
        Ok(())
    }

    pub async fn publish_webhooks_update(
        &self,
        guild_id: GuildId,
        channel_id: ChannelId,
    ) -> Result<()> {
        let gid = guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let ev = WebhooksUpdateEvent {
            metadata: meta,
            guild_id: gid,
            channel_id: channel_id.get(),
        };
        let subject = subjects::bot::webhooks_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published webhooks_update to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_stickers_update(
        &self,
        guild_id: GuildId,
        current_state: &HashMap<serenity::model::id::StickerId, serenity::model::sticker::Sticker>,
    ) -> Result<()> {
        let gid = guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let stickers: Vec<StickerInfo> = current_state
            .values()
            .map(|s| StickerInfo { id: s.id.get(), name: s.name.clone() })
            .collect();
        let ev = GuildStickersUpdateEvent { metadata: meta, guild_id: gid, stickers };
        let subject = subjects::bot::guild_stickers_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published guild_stickers_update to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_integrations_update(&self, guild_id: GuildId) -> Result<()> {
        let gid = guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let ev = GuildIntegrationsUpdateEvent { metadata: meta, guild_id: gid };
        let subject = subjects::bot::guild_integrations_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published guild_integrations_update to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_scheduled_event_user_add(
        &self,
        event: &SerenityScheduledEventUserAddEvent,
    ) -> Result<()> {
        let gid = event.guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let ev = GuildScheduledEventUserAddEvent {
            metadata: meta,
            event_id: event.scheduled_event_id.get(),
            user_id: event.user_id.get(),
            guild_id: gid,
        };
        let subject = subjects::bot::scheduled_event_user_add(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published scheduled_event_user_add to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_scheduled_event_user_remove(
        &self,
        event: &SerenityScheduledEventUserRemoveEvent,
    ) -> Result<()> {
        let gid = event.guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let ev = GuildScheduledEventUserRemoveEvent {
            metadata: meta,
            event_id: event.scheduled_event_id.get(),
            user_id: event.user_id.get(),
            guild_id: gid,
        };
        let subject = subjects::bot::scheduled_event_user_remove(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published scheduled_event_user_remove to {}", subject);
        Ok(())
    }

    pub async fn publish_category_create(&self, channel: &GuildChannel) -> Result<()> {
        let meta = EventMetadata::new(format!("dc-ch-{}", channel.id.get()), self.next_sequence());
        let ev = CategoryCreateEvent {
            metadata: meta,
            channel: Self::convert_channel(channel),
        };
        let subject = subjects::bot::category_create(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published category_create to {}", subject);
        Ok(())
    }

    pub async fn publish_category_delete(&self, channel: &GuildChannel) -> Result<()> {
        let meta = EventMetadata::new(format!("dc-ch-{}", channel.id.get()), self.next_sequence());
        let ev = CategoryDeleteEvent {
            metadata: meta,
            channel: Self::convert_channel(channel),
        };
        let subject = subjects::bot::category_delete(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published category_delete to {}", subject);
        Ok(())
    }

    pub async fn publish_bot_resume(&self) -> Result<()> {
        let meta = EventMetadata::new("dc-bot-resume".to_string(), self.next_sequence());
        let ev = BotResumeEvent { metadata: meta };
        let subject = subjects::bot::bot_resume(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published bot_resume to {}", subject);
        Ok(())
    }

    pub async fn publish_user_update(&self, new: &CurrentUser) -> Result<()> {
        let uid = new.id.get();
        let meta = EventMetadata::new(format!("dc-user-{}", uid), self.next_sequence());
        let ev = UserUpdateEvent {
            metadata: meta,
            user_id: uid,
            username: new.name.clone(),
            global_name: new.global_name.as_deref().map(String::from),
        };
        let subject = subjects::bot::user_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published user_update to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_members_chunk(
        &self,
        chunk: &SerenityGuildMembersChunkEvent,
    ) -> Result<()> {
        let gid = chunk.guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let members: Vec<FetchedMember> = chunk.members.values().map(|m| FetchedMember {
            user: DiscordUser {
                id: m.user.id.get(),
                username: m.user.name.clone(),
                global_name: m.user.global_name.as_deref().map(String::from),
                bot: m.user.bot,
            },
            guild_id: gid,
            nick: m.nick.clone(),
            roles: m.roles.iter().map(|r| r.get()).collect(),
            joined_at: m.joined_at.and_then(|t| t.to_rfc3339()),
        }).collect();
        let ev = GuildMembersChunkEvent {
            metadata: meta,
            guild_id: gid,
            chunk_index: chunk.chunk_index,
            chunk_count: chunk.chunk_count,
            members,
        };
        let subject = subjects::bot::guild_members_chunk(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published guild_members_chunk to {}", subject);
        Ok(())
    }

    pub async fn publish_voice_server_update(
        &self,
        event: &SerenityVoiceServerUpdateEvent,
    ) -> Result<()> {
        let meta = EventMetadata::new("dc-voice-server".to_string(), self.next_sequence());
        let ev = VoiceServerUpdateEvent {
            metadata: meta,
            guild_id: event.guild_id.map(|g| g.get()),
            token: event.token.clone(),
            endpoint: event.endpoint.clone(),
        };
        let subject = subjects::bot::voice_server_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published voice_server_update to {}", subject);
        Ok(())
    }

    pub async fn publish_guild_audit_log_entry(
        &self,
        guild_id: GuildId,
        entry: &serenity::model::guild::audit_log::AuditLogEntry,
    ) -> Result<()> {
        let gid = guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let info = AuditLogEntryInfo {
            id: entry.id.get(),
            user_id: entry.user_id.get(),
            target_id: entry.target_id.map(|t| t.get()),
            action_type: entry.action.num() as u32,
            reason: entry.reason.clone(),
        };
        let ev = GuildAuditLogEntryEvent { metadata: meta, guild_id: gid, entry: info };
        let subject = subjects::bot::guild_audit_log_entry(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published guild_audit_log_entry to {}", subject);
        Ok(())
    }

    pub async fn publish_thread_list_sync(
        &self,
        event: &SerenityThreadListSyncEvent,
    ) -> Result<()> {
        let gid = event.guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let ev = ThreadListSyncEvent {
            metadata: meta,
            guild_id: gid,
            channel_ids: event.channel_ids.as_ref().map(|ids| ids.iter().map(|c| c.get()).collect()),
            thread_count: event.threads.len() as u32,
        };
        let subject = subjects::bot::thread_list_sync(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published thread_list_sync to {}", subject);
        Ok(())
    }

    pub async fn publish_thread_member_update(
        &self,
        member: &serenity::model::guild::ThreadMember,
    ) -> Result<()> {
        let tid = member.id.get();
        let meta = EventMetadata::new(format!("dc-thread-{}", tid), self.next_sequence());
        let ev = ThreadMemberUpdateEvent {
            metadata: meta,
            thread_id: tid,
            guild_id: member.guild_id.map(|g| g.get()),
            user_id: member.user_id.get(),
        };
        let subject = subjects::bot::thread_member_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published thread_member_update to {}", subject);
        Ok(())
    }

    pub async fn publish_integration_create(
        &self,
        integration: &SerenityIntegration,
    ) -> Result<()> {
        let meta = EventMetadata::new(
            format!("dc-integration-{}", integration.id.get()),
            self.next_sequence(),
        );
        let ev = IntegrationCreateEvent {
            metadata: meta,
            guild_id: integration.guild_id.map(|g: GuildId| g.get()),
            integration_id: integration.id.get(),
            name: integration.name.clone(),
            kind: integration.kind.clone(),
            enabled: integration.enabled,
        };
        let subject = subjects::bot::integration_create(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published integration_create to {}", subject);
        Ok(())
    }

    pub async fn publish_integration_update(
        &self,
        integration: &SerenityIntegration,
    ) -> Result<()> {
        let meta = EventMetadata::new(
            format!("dc-integration-{}", integration.id.get()),
            self.next_sequence(),
        );
        let ev = IntegrationUpdateEvent {
            metadata: meta,
            guild_id: integration.guild_id.map(|g: GuildId| g.get()),
            integration_id: integration.id.get(),
            name: integration.name.clone(),
            kind: integration.kind.clone(),
            enabled: integration.enabled,
        };
        let subject = subjects::bot::integration_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published integration_update to {}", subject);
        Ok(())
    }

    pub async fn publish_integration_delete(
        &self,
        integration_id: IntegrationId,
        guild_id: GuildId,
        application_id: Option<ApplicationId>,
    ) -> Result<()> {
        let gid = guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let ev = IntegrationDeleteEvent {
            metadata: meta,
            guild_id: gid,
            integration_id: integration_id.get(),
            application_id: application_id.map(|a| a.get()),
        };
        let subject = subjects::bot::integration_delete(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published integration_delete to {}", subject);
        Ok(())
    }

    pub async fn publish_voice_channel_status_update(
        &self,
        old: Option<String>,
        status: Option<String>,
        id: ChannelId,
        guild_id: GuildId,
    ) -> Result<()> {
        let gid = guild_id.get();
        let meta = EventMetadata::new(format!("dc-vc-{}", id.get()), self.next_sequence());
        let ev = VoiceChannelStatusUpdateEvent {
            metadata: meta,
            channel_id: id.get(),
            guild_id: gid,
            old_status: old,
            new_status: status,
        };
        let subject = subjects::bot::voice_channel_status_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published voice_channel_status_update to {}", subject);
        Ok(())
    }

    pub async fn publish_automod_rule_create(&self, rule: &SerenityAutoModRule) -> Result<()> {
        let gid = rule.guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let ev = AutoModRuleCreateEvent {
            metadata: meta,
            guild_id: gid,
            rule_id: rule.id.get(),
            name: rule.name.clone(),
            enabled: rule.enabled,
        };
        let subject = subjects::bot::automod_rule_create(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published automod_rule_create to {}", subject);
        Ok(())
    }

    pub async fn publish_automod_rule_update(&self, rule: &SerenityAutoModRule) -> Result<()> {
        let gid = rule.guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let ev = AutoModRuleUpdateEvent {
            metadata: meta,
            guild_id: gid,
            rule_id: rule.id.get(),
            name: rule.name.clone(),
            enabled: rule.enabled,
        };
        let subject = subjects::bot::automod_rule_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published automod_rule_update to {}", subject);
        Ok(())
    }

    pub async fn publish_automod_rule_delete(&self, rule: &SerenityAutoModRule) -> Result<()> {
        let gid = rule.guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let ev = AutoModRuleDeleteEvent {
            metadata: meta,
            guild_id: gid,
            rule_id: rule.id.get(),
            name: rule.name.clone(),
        };
        let subject = subjects::bot::automod_rule_delete(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published automod_rule_delete to {}", subject);
        Ok(())
    }

    pub async fn publish_automod_action_execution(
        &self,
        execution: &SerenityActionExecution,
    ) -> Result<()> {
        let gid = execution.guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let ev = AutoModActionExecutionEvent {
            metadata: meta,
            guild_id: gid,
            rule_id: execution.rule_id.get(),
            user_id: execution.user_id.get(),
            channel_id: execution.channel_id.map(|c| c.get()),
            message_id: execution.message_id.map(|m| m.get()),
            matched_keyword: execution.matched_keyword.clone(),
        };
        let subject = subjects::bot::automod_action_execution(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published automod_action_execution to {}", subject);
        Ok(())
    }

    pub async fn publish_command_permissions_update(
        &self,
        permissions: &SerenityCommandPermissions,
    ) -> Result<()> {
        let gid = permissions.guild_id.get();
        let meta = EventMetadata::new(format!("dc-guild-{}", gid), self.next_sequence());
        let ev = CommandPermissionsUpdateEvent {
            metadata: meta,
            guild_id: gid,
            command_id: permissions.id.get(),
            application_id: permissions.application_id.get(),
        };
        let subject = subjects::bot::command_permissions_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published command_permissions_update to {}", subject);
        Ok(())
    }

    pub async fn publish_entitlement_create(&self, entitlement: &SerenityEntitlement) -> Result<()> {
        let meta = EventMetadata::new(
            format!("dc-entitlement-{}", entitlement.id.get()),
            self.next_sequence(),
        );
        let ev = EntitlementCreateEvent {
            metadata: meta,
            entitlement_id: entitlement.id.get(),
            sku_id: entitlement.sku_id.get(),
            application_id: entitlement.application_id.get(),
            user_id: entitlement.user_id.map(|u| u.get()),
            guild_id: entitlement.guild_id.map(|g| g.get()),
        };
        let subject = subjects::bot::entitlement_create(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published entitlement_create to {}", subject);
        Ok(())
    }

    pub async fn publish_entitlement_update(&self, entitlement: &SerenityEntitlement) -> Result<()> {
        let meta = EventMetadata::new(
            format!("dc-entitlement-{}", entitlement.id.get()),
            self.next_sequence(),
        );
        let ev = EntitlementUpdateEvent {
            metadata: meta,
            entitlement_id: entitlement.id.get(),
            sku_id: entitlement.sku_id.get(),
            application_id: entitlement.application_id.get(),
            user_id: entitlement.user_id.map(|u| u.get()),
            guild_id: entitlement.guild_id.map(|g| g.get()),
        };
        let subject = subjects::bot::entitlement_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published entitlement_update to {}", subject);
        Ok(())
    }

    pub async fn publish_entitlement_delete(&self, entitlement: &SerenityEntitlement) -> Result<()> {
        let meta = EventMetadata::new(
            format!("dc-entitlement-{}", entitlement.id.get()),
            self.next_sequence(),
        );
        let ev = EntitlementDeleteEvent {
            metadata: meta,
            entitlement_id: entitlement.id.get(),
            sku_id: entitlement.sku_id.get(),
            application_id: entitlement.application_id.get(),
            user_id: entitlement.user_id.map(|u| u.get()),
            guild_id: entitlement.guild_id.map(|g| g.get()),
        };
        let subject = subjects::bot::entitlement_delete(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published entitlement_delete to {}", subject);
        Ok(())
    }

    pub async fn publish_poll_vote_add(&self, event: &SerenityPollVoteAddEvent) -> Result<()> {
        let meta = EventMetadata::new(
            format!("dc-msg-{}", event.message_id.get()),
            self.next_sequence(),
        );
        let ev = PollVoteAddEvent {
            metadata: meta,
            user_id: event.user_id.get(),
            channel_id: event.channel_id.get(),
            message_id: event.message_id.get(),
            guild_id: event.guild_id.map(|g| g.get()),
            answer_id: event.answer_id.get(),
        };
        let subject = subjects::bot::poll_vote_add(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published poll_vote_add to {}", subject);
        Ok(())
    }

    pub async fn publish_poll_vote_remove(&self, event: &SerenityPollVoteRemoveEvent) -> Result<()> {
        let meta = EventMetadata::new(
            format!("dc-msg-{}", event.message_id.get()),
            self.next_sequence(),
        );
        let ev = PollVoteRemoveEvent {
            metadata: meta,
            user_id: event.user_id.get(),
            channel_id: event.channel_id.get(),
            message_id: event.message_id.get(),
            guild_id: event.guild_id.map(|g| g.get()),
            answer_id: event.answer_id.get(),
        };
        let subject = subjects::bot::poll_vote_remove(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published poll_vote_remove to {}", subject);
        Ok(())
    }

    pub async fn publish_reaction_remove_emoji(
        &self,
        reaction: &serenity::model::channel::Reaction,
    ) -> Result<()> {
        let cid = reaction.channel_id.get();
        let meta = EventMetadata::new(format!("dc-ch-{}", cid), self.next_sequence());
        let emoji = convert_reaction_type(&reaction.emoji);
        let ev = ReactionRemoveEmojiEvent {
            metadata: meta,
            channel_id: cid,
            message_id: reaction.message_id.get(),
            guild_id: reaction.guild_id.map(|g| g.get()),
            emoji_name: if emoji.name.is_empty() { None } else { Some(emoji.name) },
            emoji_id: emoji.id,
        };
        let subject = subjects::bot::reaction_remove_emoji(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published reaction_remove_emoji to {}", subject);
        Ok(())
    }

    pub async fn publish_presence_replace(&self, presences: &[Presence]) -> Result<()> {
        let meta = EventMetadata::new("dc-bot-presence-replace".to_string(), self.next_sequence());
        let ev = PresenceReplaceEvent {
            metadata: meta,
            count: presences.len() as u32,
        };
        let subject = subjects::bot::presence_replace(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published presence_replace to {}", subject);
        Ok(())
    }

    pub async fn publish_shard_stage_update(&self, event: &SerenityShardStageUpdateEvent) -> Result<()> {
        let shard_id = event.shard_id.0 as u64;
        let meta = EventMetadata::new(format!("dc-shard-{}", shard_id), self.next_sequence());
        let stage_str = |s: &ConnectionStage| -> &'static str {
            match s {
                ConnectionStage::Connected => "connected",
                ConnectionStage::Connecting => "connecting",
                ConnectionStage::Disconnected => "disconnected",
                ConnectionStage::Handshake => "handshake",
                ConnectionStage::Identifying => "identifying",
                ConnectionStage::Resuming => "resuming",
                _ => "unknown",
            }
        };
        let ev = ShardStageUpdateEvent {
            metadata: meta,
            shard_id,
            old: stage_str(&event.old).to_string(),
            new: stage_str(&event.new).to_string(),
        };
        let subject = subjects::bot::shard_stage_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published shard_stage_update to {}", subject);
        Ok(())
    }

    pub async fn publish_soundboard_sounds(&self, event: &SerenitySoundboardSoundsEvent) -> Result<()> {
        let gid = event.guild_id.get();
        let meta = EventMetadata::new(member_session_id(gid), self.next_sequence());
        let ev = SoundboardSoundsEvent {
            metadata: meta,
            guild_id: gid,
            sounds: event.soundboard_sounds.iter().map(convert_soundboard).collect(),
        };
        let subject = subjects::bot::soundboard_sounds(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published soundboard_sounds to {}", subject);
        Ok(())
    }

    pub async fn publish_soundboard_sound_create(&self, event: &SerenitySoundboardSoundCreateEvent) -> Result<()> {
        let sound = &event.soundboard;
        let meta = EventMetadata::new(format!("dc-sound-{}", sound.id.get()), self.next_sequence());
        let ev = SoundboardSoundCreateEvent {
            metadata: meta,
            sound: convert_soundboard(sound),
        };
        let subject = subjects::bot::soundboard_sound_create(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published soundboard_sound_create to {}", subject);
        Ok(())
    }

    pub async fn publish_soundboard_sound_update(&self, event: &SerenitySoundboardSoundUpdateEvent) -> Result<()> {
        let sound = &event.soundboard;
        let meta = EventMetadata::new(format!("dc-sound-{}", sound.id.get()), self.next_sequence());
        let ev = SoundboardSoundUpdateEvent {
            metadata: meta,
            sound: convert_soundboard(sound),
        };
        let subject = subjects::bot::soundboard_sound_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published soundboard_sound_update to {}", subject);
        Ok(())
    }

    pub async fn publish_soundboard_sounds_update(&self, event: &SerenitySoundboardSoundsUpdateEvent) -> Result<()> {
        let gid = event.guild_id.get();
        let meta = EventMetadata::new(member_session_id(gid), self.next_sequence());
        let ev = SoundboardSoundsUpdateEvent {
            metadata: meta,
            guild_id: gid,
            sounds: event.soundboard_sounds.iter().map(convert_soundboard).collect(),
        };
        let subject = subjects::bot::soundboard_sounds_update(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published soundboard_sounds_update to {}", subject);
        Ok(())
    }

    pub async fn publish_soundboard_sound_delete(&self, event: &SerenitySoundboardSoundDeleteEvent) -> Result<()> {
        let gid = event.guild_id.get();
        let meta = EventMetadata::new(member_session_id(gid), self.next_sequence());
        let ev = SoundboardSoundDeleteEvent {
            metadata: meta,
            guild_id: gid,
            sound_id: event.sound_id.get(),
        };
        let subject = subjects::bot::soundboard_sound_delete(self.prefix());
        self.publisher.publish(&subject, &ev).await?;
        debug!("Published soundboard_sound_delete to {}", subject);
        Ok(())
    }

}

fn convert_soundboard(sound: &SerenitySoundboard) -> SoundInfo {
    SoundInfo {
        id: sound.id.get(),
        name: sound.name.clone(),
        volume: sound.volume,
        emoji_id: sound.emoji_id.map(|e| e.get()),
        emoji_name: sound.emoji_name.clone(),
        guild_id: sound.guild_id.map(|g| g.get()),
        available: sound.available,
    }
}

fn convert_reaction_type(reaction: &ReactionType) -> Emoji {
    match reaction {
        ReactionType::Unicode(s) => Emoji {
            id: None,
            name: s.clone(),
            animated: false,
        },
        ReactionType::Custom { animated, id, name } => Emoji {
            id: Some(id.get()),
            name: name.clone().unwrap_or_default(),
            animated: *animated,
        },
        _ => Emoji {
            id: None,
            name: String::new(),
            animated: false,
        },
    }
}
