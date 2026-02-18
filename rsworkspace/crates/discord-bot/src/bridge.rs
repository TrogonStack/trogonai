//! Bridge between Discord (serenity) and NATS
//!
//! Converts serenity events into discord-types events and publishes them to NATS.

#[path = "bridge_tests.rs"]
mod bridge_tests;

#[path = "bridge_nats_tests.rs"]
mod bridge_nats_tests;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::Result;
use discord_nats::{subjects, MessagePublisher};
use discord_types::{
    events::*,
    session::{member_session_id, session_id},
    types::{
        Attachment, ChannelType, CommandOption, CommandOptionValue, ComponentType, DiscordChannel,
        DiscordGuild, DiscordMember, DiscordMessage, DiscordRole, DiscordUser, Embed, EmbedAuthor,
        EmbedField, EmbedFooter, EmbedMedia, Emoji, ModalInput, VoiceState as BridgeVoiceState,
    },
    AccessConfig,
};
use serenity::model::application::{
    CommandInteraction, ComponentInteraction, ComponentInteractionDataKind,
    ModalInteraction, ResolvedValue,
};
use serenity::model::channel::GuildChannel;
use serenity::model::event::TypingStartEvent as SerenityTypingStartEvent;
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
    ) -> Self {
        Self {
            publisher: MessagePublisher::new(client, prefix),
            access_config,
            sequence: Arc::new(AtomicU64::new(0)),
            bot_user_id: Arc::new(AtomicU64::new(0)),
            presence_enabled,
            guild_commands_guild_id,
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

    /// Check if a user has DM access
    pub fn check_dm_access(&self, user_id: u64) -> bool {
        self.access_config.can_access_dm(user_id)
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

        let embeds = msg
            .embeds
            .iter()
            .map(|e| Embed {
                title: e.title.clone(),
                description: e.description.clone(),
                url: e.url.clone(),
                fields: e
                    .fields
                    .iter()
                    .map(|f| EmbedField {
                        name: f.name.clone(),
                        value: f.value.clone(),
                        inline: f.inline,
                    })
                    .collect(),
                color: e.colour.map(|c| c.0),
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
            })
            .collect();

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
                .map(|embeds| {
                    embeds
                        .iter()
                        .map(|e| Embed {
                            title: e.title.clone(),
                            description: e.description.clone(),
                            url: e.url.clone(),
                            fields: e
                                .fields
                                .iter()
                                .map(|f| EmbedField {
                                    name: f.name.clone(),
                                    value: f.value.clone(),
                                    inline: f.inline,
                                })
                                .collect(),
                            color: e.colour.map(|c| c.0),
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
                        })
                        .collect()
                })
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
            .map(|opt| {
                let value = match &opt.value {
                    ResolvedValue::Boolean(b) => CommandOptionValue::Boolean(*b),
                    ResolvedValue::Integer(i) => CommandOptionValue::Integer(*i),
                    ResolvedValue::Number(n) => CommandOptionValue::Number(*n),
                    ResolvedValue::String(s) => CommandOptionValue::String(s.to_string()),
                    ResolvedValue::User(u, _) => CommandOptionValue::User(u.id.get()),
                    ResolvedValue::Channel(c) => CommandOptionValue::Channel(c.id.get()),
                    ResolvedValue::Role(r) => CommandOptionValue::Role(r.id.get()),
                    _ => CommandOptionValue::String(String::new()),
                };
                CommandOption {
                    name: opt.name.to_string(),
                    value,
                }
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

        let component_type = match interaction.data.kind {
            ComponentInteractionDataKind::Button => ComponentType::Button,
            ComponentInteractionDataKind::StringSelect { .. } => ComponentType::StringSelect,
            ComponentInteractionDataKind::UserSelect { .. } => ComponentType::UserSelect,
            ComponentInteractionDataKind::RoleSelect { .. } => ComponentType::RoleSelect,
            ComponentInteractionDataKind::MentionableSelect { .. } => {
                ComponentType::MentionableSelect
            }
            ComponentInteractionDataKind::ChannelSelect { .. } => ComponentType::ChannelSelect,
            _ => ComponentType::Unknown,
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
