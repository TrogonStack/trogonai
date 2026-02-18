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
        Attachment, ChannelType, CommandOption, CommandOptionValue, ComponentType, DiscordMember,
        DiscordMessage, DiscordUser, Embed, EmbedField, Emoji,
    },
    AccessConfig,
};
use serenity::model::application::{
    CommandInteraction, ComponentInteraction, ComponentInteractionDataKind, ResolvedValue,
};
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
}

impl TypeMapKey for DiscordBridge {
    type Value = Arc<DiscordBridge>;
}

impl DiscordBridge {
    /// Create a new bridge
    pub fn new(client: async_nats::Client, prefix: String, access_config: AccessConfig) -> Self {
        Self {
            publisher: MessagePublisher::new(client, prefix),
            access_config,
            sequence: Arc::new(AtomicU64::new(0)),
            bot_user_id: Arc::new(AtomicU64::new(0)),
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
