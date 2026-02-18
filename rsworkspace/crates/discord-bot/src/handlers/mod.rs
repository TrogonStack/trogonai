//! Serenity event handler implementation

use serenity::async_trait;
use serenity::builder::{CreateCommand, CreateCommandOption};
use serenity::model::application::{Command, CommandOptionType, Interaction};
use serenity::model::channel::{GuildChannel, Message};
use serenity::model::event::{MessageUpdateEvent, TypingStartEvent};
use serenity::model::gateway::{Presence, Ready};
use serenity::model::guild::{Guild, Member, PartialGuild, Role, UnavailableGuild};
use serenity::model::id::{ChannelId, GuildId, MessageId, RoleId};
use serenity::model::user::User;
use serenity::model::voice::VoiceState;
use serenity::prelude::*;
use tracing::{debug, error, info, warn};

use crate::bridge::DiscordBridge;
use crate::health::AppState;

pub struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        info!(
            "Discord bot connected as {}#{:04}",
            ready.user.name,
            ready.user.discriminator.map_or(0, |d| d.get())
        );

        {
            let data = ctx.data.read().await;
            if let Some(health_state) = data.get::<AppState>() {
                health_state.set_bot_username(ready.user.name.clone()).await;
            }
            if let Some(bridge) = data.get::<DiscordBridge>() {
                bridge.set_bot_user_id(ready.user.id.get());
                if let Err(e) = bridge.publish_bot_ready(&ready).await {
                    error!("Failed to publish bot_ready: {}", e);
                }
            }
        }

        // Register slash commands globally (takes ~1 hour to propagate after first deploy)
        let commands = vec![
            CreateCommand::new("ping").description("Check if the bot is alive"),
            CreateCommand::new("help").description("Show available commands"),
            CreateCommand::new("status").description("Show agent status"),
            CreateCommand::new("clear").description("Clear your conversation history"),
            CreateCommand::new("forget")
                .description("Remove the last message exchange from memory"),
            CreateCommand::new("summarize").description("Summarize your conversation so far"),
            CreateCommand::new("ask")
                .description("Ask the AI a question")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "question",
                        "The question to ask",
                    )
                    .required(true),
                ),
        ];

        match Command::set_global_commands(&ctx.http, commands).await {
            Ok(cmds) => info!("Registered {} slash commands globally", cmds.len()),
            Err(e) => warn!("Failed to register slash commands: {}", e),
        }
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

        // Access control: check guild or DM; admins bypass guild restrictions
        if let Some(guild_id) = msg.guild_id {
            if !bridge.check_guild_access(guild_id.get()) && !bridge.is_admin(msg.author.id.get()) {
                debug!(
                    user = msg.author.id.get(),
                    guild = guild_id.get(),
                    "Message dropped: guild not in allowlist"
                );
                return;
            }
            if !bridge.check_require_mention(&msg.mentions) && !bridge.is_admin(msg.author.id.get())
            {
                debug!(
                    user = msg.author.id.get(),
                    channel = msg.channel_id.get(),
                    "Message dropped: require_mention is set and bot was not mentioned"
                );
                return;
            }
            if !bridge.check_channel_access(msg.channel_id.get())
                && !bridge.is_admin(msg.author.id.get())
            {
                debug!(
                    user = msg.author.id.get(),
                    channel = msg.channel_id.get(),
                    "Message dropped: channel not in allowlist"
                );
                return;
            }
        } else {
            // DM
            if !bridge.check_dm_access(msg.author.id.get()) {
                debug!(
                    user = msg.author.id.get(),
                    "Message dropped: DM not allowed for this user"
                );
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
                // Access control; admins bypass guild restrictions
                if let Some(guild_id) = cmd.guild_id {
                    if !bridge.check_guild_access(guild_id.get())
                        && !bridge.is_admin(cmd.user.id.get())
                    {
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
                // Access control; admins bypass guild restrictions
                if let Some(guild_id) = comp.guild_id {
                    if !bridge.check_guild_access(guild_id.get())
                        && !bridge.is_admin(comp.user.id.get())
                    {
                        return;
                    }
                } else if !bridge.check_dm_access(comp.user.id.get()) {
                    return;
                }

                if let Err(e) = bridge.publish_component_interaction(&comp).await {
                    error!("Failed to publish component_interaction: {}", e);
                }
            }
            Interaction::Autocomplete(cmd) => {
                if let Err(e) = bridge.publish_autocomplete(&cmd).await {
                    error!("Failed to publish autocomplete: {}", e);
                }
            }
            Interaction::Modal(modal) => {
                if let Err(e) = bridge.publish_modal_submit(&modal).await {
                    error!("Failed to publish modal_submit: {}", e);
                }
            }
            _ => {}
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

    async fn typing_start(&self, ctx: Context, event: TypingStartEvent) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Err(e) = bridge.publish_typing_start(&event).await {
            debug!("Failed to publish typing_start: {}", e);
        }
    }

    async fn voice_state_update(
        &self,
        ctx: Context,
        old: Option<VoiceState>,
        new: VoiceState,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Err(e) = bridge.publish_voice_state_update(old.as_ref(), &new).await {
            error!("Failed to publish voice_state_update: {}", e);
        }
    }

    async fn guild_create(&self, ctx: Context, guild: Guild, _is_new: Option<bool>) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        let member_count = guild.member_count;
        if let Err(e) = bridge.publish_guild_create(&guild, member_count).await {
            error!("Failed to publish guild_create: {}", e);
        }
    }

    async fn guild_update(&self, ctx: Context, _old: Option<Guild>, new: PartialGuild) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Err(e) = bridge.publish_guild_update(&new).await {
            error!("Failed to publish guild_update: {}", e);
        }
    }

    async fn guild_delete(
        &self,
        ctx: Context,
        incomplete: UnavailableGuild,
        _full: Option<Guild>,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Err(e) = bridge.publish_guild_delete(&incomplete).await {
            error!("Failed to publish guild_delete: {}", e);
        }
    }

    async fn channel_create(&self, ctx: Context, channel: GuildChannel) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(channel.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_channel_create(&channel).await {
            error!("Failed to publish channel_create: {}", e);
        }
    }

    async fn channel_update(&self, ctx: Context, _old: Option<GuildChannel>, new: GuildChannel) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(new.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_channel_update(&new).await {
            error!("Failed to publish channel_update: {}", e);
        }
    }

    async fn channel_delete(
        &self,
        ctx: Context,
        channel: GuildChannel,
        _messages: Option<Vec<Message>>,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(channel.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge
            .publish_channel_delete(channel.id.get(), channel.guild_id.get())
            .await
        {
            error!("Failed to publish channel_delete: {}", e);
        }
    }

    async fn guild_role_create(&self, ctx: Context, new: Role) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        let guild_id = new.guild_id.get();
        if !bridge.check_guild_access(guild_id) {
            return;
        }
        if let Err(e) = bridge.publish_role_create(guild_id, &new).await {
            error!("Failed to publish role_create: {}", e);
        }
    }

    async fn guild_role_update(&self, ctx: Context, _old: Option<Role>, new: Role) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        let guild_id = new.guild_id.get();
        if !bridge.check_guild_access(guild_id) {
            return;
        }
        if let Err(e) = bridge.publish_role_update(guild_id, &new).await {
            error!("Failed to publish role_update: {}", e);
        }
    }

    async fn guild_role_delete(
        &self,
        ctx: Context,
        guild_id: GuildId,
        role_id: RoleId,
        _role: Option<Role>,
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
        if let Err(e) = bridge.publish_role_delete(gid, role_id.get()).await {
            error!("Failed to publish role_delete: {}", e);
        }
    }

    async fn presence_update(&self, ctx: Context, new_data: Presence) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.presence_enabled {
            return;
        }
        if let Err(e) = bridge.publish_presence_update(&new_data).await {
            debug!("Failed to publish presence_update: {}", e);
        }
    }
}
