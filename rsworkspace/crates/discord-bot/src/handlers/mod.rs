//! Serenity event handler implementation

use serenity::async_trait;
use serenity::model::application::Interaction;
use serenity::model::channel::Message;
use serenity::model::event::MessageUpdateEvent;
use serenity::model::gateway::Ready;
use serenity::model::guild::Member;
use serenity::model::id::{ChannelId, GuildId, MessageId};
use serenity::model::user::User;
use serenity::prelude::*;
use tracing::{error, info};

use crate::bridge::DiscordBridge;

pub struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        info!(
            "Discord bot connected as {}#{:04}",
            ready.user.name,
            ready.user.discriminator.map_or(0, |d| d.get())
        );
    }

    async fn message(&self, ctx: Context, msg: Message) {
        // Skip bot messages
        if msg.author.bot {
            return;
        }

        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => {
                    error!("DiscordBridge not found in context data");
                    return;
                }
            }
        };

        // Access control: check guild or DM
        if let Some(guild_id) = msg.guild_id {
            if !bridge.check_guild_access(guild_id.get()) {
                return;
            }
        } else {
            // DM
            if !bridge.check_dm_access(msg.author.id.get()) {
                return;
            }
        }

        if let Err(e) = bridge.publish_message_created(&msg).await {
            error!("Failed to publish message_created: {}", e);
        }
    }

    async fn message_update(
        &self,
        ctx: Context,
        _old: Option<Message>,
        _new: Option<Message>,
        event: MessageUpdateEvent,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };

        // Access control
        if let Some(guild_id) = event.guild_id {
            if !bridge.check_guild_access(guild_id.get()) {
                return;
            }
        }

        if let Err(e) = bridge.publish_message_updated(&event).await {
            error!("Failed to publish message_updated: {}", e);
        }
    }

    async fn message_delete(
        &self,
        ctx: Context,
        channel_id: ChannelId,
        deleted_message_id: MessageId,
        guild_id: Option<GuildId>,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };

        // Access control
        if let Some(gid) = guild_id {
            if !bridge.check_guild_access(gid.get()) {
                return;
            }
        }

        if let Err(e) = bridge
            .publish_message_deleted(channel_id, guild_id, deleted_message_id)
            .await
        {
            error!("Failed to publish message_deleted: {}", e);
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };

        match interaction {
            Interaction::Command(cmd) => {
                // Access control
                if let Some(guild_id) = cmd.guild_id {
                    if !bridge.check_guild_access(guild_id.get()) {
                        return;
                    }
                } else if !bridge.check_dm_access(cmd.user.id.get()) {
                    return;
                }

                if let Err(e) = bridge.publish_slash_command(&cmd).await {
                    error!("Failed to publish slash_command: {}", e);
                }
            }
            Interaction::Component(comp) => {
                // Access control
                if let Some(guild_id) = comp.guild_id {
                    if !bridge.check_guild_access(guild_id.get()) {
                        return;
                    }
                } else if !bridge.check_dm_access(comp.user.id.get()) {
                    return;
                }

                if let Err(e) = bridge.publish_component_interaction(&comp).await {
                    error!("Failed to publish component_interaction: {}", e);
                }
            }
            _ => {
                // Other interaction types (autocomplete, modal, ping) not handled
            }
        }
    }

    async fn reaction_add(&self, ctx: Context, add_reaction: serenity::model::channel::Reaction) {
        // Skip bot reactions
        if let Some(user_id) = add_reaction.user_id {
            // We can't easily check if it's a bot here without an API call.
            // The bridge will handle this gracefully.
            let _ = user_id;
        }

        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };

        // Access control
        if let Some(guild_id) = add_reaction.guild_id {
            if !bridge.check_guild_access(guild_id.get()) {
                return;
            }
        }

        if let Err(e) = bridge.publish_reaction_add(&add_reaction).await {
            error!("Failed to publish reaction_add: {}", e);
        }
    }

    async fn reaction_remove(
        &self,
        ctx: Context,
        removed_reaction: serenity::model::channel::Reaction,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };

        // Access control
        if let Some(guild_id) = removed_reaction.guild_id {
            if !bridge.check_guild_access(guild_id.get()) {
                return;
            }
        }

        if let Err(e) = bridge.publish_reaction_remove(&removed_reaction).await {
            error!("Failed to publish reaction_remove: {}", e);
        }
    }

    async fn guild_member_addition(&self, ctx: Context, new_member: Member) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };

        let guild_id = new_member.guild_id.get();

        if !bridge.check_guild_access(guild_id) {
            return;
        }

        if let Err(e) = bridge.publish_guild_member_add(guild_id, &new_member).await {
            error!("Failed to publish guild_member_add: {}", e);
        }
    }

    async fn guild_member_removal(
        &self,
        ctx: Context,
        guild_id: GuildId,
        user: User,
        _member_data: Option<Member>,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };

        let gid = guild_id.get();

        if !bridge.check_guild_access(gid) {
            return;
        }

        if let Err(e) = bridge.publish_guild_member_remove(gid, &user).await {
            error!("Failed to publish guild_member_remove: {}", e);
        }
    }
}
