//! Serenity event handler implementation

use serenity::async_trait;
use serenity::builder::{CreateCommand, CreateCommandOption};
use serenity::model::application::{Command, CommandOptionType, Interaction};
use serenity::model::channel::{GuildChannel, Message};
use serenity::model::application::CommandPermissions;
use serenity::model::event::{
    ChannelPinsUpdateEvent, GuildMembersChunkEvent, GuildScheduledEventUserAddEvent,
    GuildScheduledEventUserRemoveEvent, MessagePollVoteAddEvent, MessagePollVoteRemoveEvent,
    MessageUpdateEvent, ThreadListSyncEvent, TypingStartEvent, VoiceServerUpdateEvent,
};
use serenity::model::gateway::{Presence, Ready};
use serenity::model::guild::{
    Guild, Member, PartialGuild, Role, ScheduledEvent, ThreadMember, UnavailableGuild,
};
use serenity::model::guild::automod::{ActionExecution, Rule as AutoModRule};
use serenity::model::guild::Integration;
use serenity::model::id::{ApplicationId, ChannelId, GuildId, IntegrationId, MessageId, RoleId};
use serenity::model::monetization::Entitlement;
use serenity::model::user::{CurrentUser, User};
use serenity::model::voice::VoiceState;
use serenity::prelude::*;
use std::collections::HashMap;
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

        // Register slash commands — guild-level (instant) or global (~1 hour propagation)
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

        let guild_commands_guild_id = {
            let data = ctx.data.read().await;
            data.get::<DiscordBridge>()
                .and_then(|b| b.guild_commands_guild_id)
        };

        if let Some(guild_id) = guild_commands_guild_id {
            match GuildId::new(guild_id).set_commands(&ctx.http, commands).await {
                Ok(cmds) => info!(
                    "Registered {} slash commands for guild {}",
                    cmds.len(),
                    guild_id
                ),
                Err(e) => warn!("Failed to register guild slash commands: {}", e),
            }
        } else {
            match Command::set_global_commands(&ctx.http, commands).await {
                Ok(cmds) => info!("Registered {} slash commands globally", cmds.len()),
                Err(e) => warn!("Failed to register slash commands: {}", e),
            }
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
        // Skip reactions from bot accounts (check cache; uncached users pass through).
        if let Some(user_id) = add_reaction.user_id {
            if ctx.cache.user(user_id).map(|u| u.bot).unwrap_or(false) {
                return;
            }
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
        // Skip reactions from bot accounts (check cache; uncached users pass through).
        if let Some(user_id) = removed_reaction.user_id {
            if ctx.cache.user(user_id).map(|u| u.bot).unwrap_or(false) {
                return;
            }
        }

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

    async fn message_delete_bulk(
        &self,
        ctx: Context,
        channel_id: ChannelId,
        message_ids: Vec<MessageId>,
        guild_id: Option<GuildId>,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };

        if let Err(e) = bridge
            .publish_message_bulk_deleted(channel_id, guild_id, message_ids)
            .await
        {
            error!("failed to publish message_delete_bulk: {e}");
        }
    }

    async fn guild_member_update(
        &self,
        ctx: Context,
        _old: Option<Member>,
        new: Option<Member>,
        _event: serenity::model::event::GuildMemberUpdateEvent,
    ) {
        let Some(member) = new else { return };
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };

        let nick = member.nick.clone();
        let roles: Vec<u64> = member.roles.iter().map(|r| r.get()).collect();
        if let Err(e) = bridge
            .publish_guild_member_updated(member.guild_id, &member.user, nick, roles)
            .await
        {
            error!("failed to publish guild_member_update: {e}");
        }
    }

    async fn invite_create(
        &self,
        ctx: Context,
        data: serenity::model::event::InviteCreateEvent,
    ) {
        let bridge = {
            let d = ctx.data.read().await;
            match d.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Some(guild_id) = data.guild_id {
            if !bridge.check_guild_access(guild_id.get()) {
                return;
            }
        }
        if let Err(e) = bridge.publish_invite_create(&data).await {
            error!("Failed to publish invite_create: {}", e);
        }
    }

    async fn invite_delete(
        &self,
        ctx: Context,
        data: serenity::model::event::InviteDeleteEvent,
    ) {
        let bridge = {
            let d = ctx.data.read().await;
            match d.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Some(guild_id) = data.guild_id {
            if !bridge.check_guild_access(guild_id.get()) {
                return;
            }
        }
        if let Err(e) = bridge.publish_invite_delete(&data).await {
            error!("Failed to publish invite_delete: {}", e);
        }
    }

    async fn thread_create(&self, ctx: Context, thread: GuildChannel) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(thread.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_thread_created(&thread).await {
            error!("Failed to publish thread_created: {}", e);
        }
    }

    async fn thread_update(&self, ctx: Context, _old: Option<GuildChannel>, new: GuildChannel) {
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
        if let Err(e) = bridge.publish_thread_updated(&new).await {
            error!("Failed to publish thread_updated: {}", e);
        }
    }

    async fn thread_delete(
        &self,
        ctx: Context,
        thread: serenity::model::channel::PartialGuildChannel,
        _full: Option<GuildChannel>,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        let guild_id = thread.guild_id.get();
        if !bridge.check_guild_access(guild_id) {
            return;
        }
        let parent_id = Some(thread.parent_id.get());
        if let Err(e) = bridge
            .publish_thread_deleted(thread.id.get(), guild_id, parent_id)
            .await
        {
            error!("Failed to publish thread_deleted: {}", e);
        }
    }

    async fn thread_members_update(
        &self,
        ctx: Context,
        event: serenity::model::event::ThreadMembersUpdateEvent,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        let guild_id = event.guild_id.get();
        if !bridge.check_guild_access(guild_id) {
            return;
        }
        let thread_id = event.id.get();

        // Publish member additions
        let added: Vec<u64> = event
            .added_members
            .iter()
            .map(|m| m.user_id.get())
            .collect();
        if !added.is_empty() {
            if let Err(e) = bridge
                .publish_thread_member_add(thread_id, guild_id, added)
                .await
            {
                error!("Failed to publish thread_member_add: {}", e);
            }
        }

        // Publish member removals
        let removed: Vec<u64> = event.removed_member_ids.iter().map(|id| id.get()).collect();
        if !removed.is_empty() {
            if let Err(e) = bridge
                .publish_thread_member_remove(thread_id, guild_id, removed)
                .await
            {
                error!("Failed to publish thread_member_remove: {}", e);
            }
        }
    }

    async fn stage_instance_create(
        &self,
        ctx: Context,
        stage: serenity::model::channel::StageInstance,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(stage.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_stage_instance_create(&stage).await {
            error!("Failed to publish stage_instance_create: {}", e);
        }
    }

    async fn stage_instance_update(
        &self,
        ctx: Context,
        stage: serenity::model::channel::StageInstance,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(stage.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_stage_instance_update(&stage).await {
            error!("Failed to publish stage_instance_update: {}", e);
        }
    }

    async fn stage_instance_delete(
        &self,
        ctx: Context,
        stage: serenity::model::channel::StageInstance,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(stage.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_stage_instance_delete(&stage).await {
            error!("Failed to publish stage_instance_delete: {}", e);
        }
    }

    async fn guild_ban_addition(&self, ctx: Context, guild_id: GuildId, banned_user: User) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_guild_ban_add(guild_id, &banned_user).await {
            error!("Failed to publish guild_ban_add: {}", e);
        }
    }

    async fn guild_ban_removal(&self, ctx: Context, guild_id: GuildId, unbanned_user: User) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_guild_ban_remove(guild_id, &unbanned_user).await {
            error!("Failed to publish guild_ban_remove: {}", e);
        }
    }

    async fn guild_emojis_update(
        &self,
        ctx: Context,
        guild_id: GuildId,
        current_state: HashMap<serenity::model::id::EmojiId, serenity::model::guild::Emoji>,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_guild_emojis_update(guild_id, &current_state).await {
            error!("Failed to publish guild_emojis_update: {}", e);
        }
    }

    async fn guild_scheduled_event_create(&self, ctx: Context, event: ScheduledEvent) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(event.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_guild_scheduled_event_create(&event).await {
            error!("Failed to publish guild_scheduled_event_create: {}", e);
        }
    }

    async fn guild_scheduled_event_update(&self, ctx: Context, event: ScheduledEvent) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(event.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_guild_scheduled_event_update(&event).await {
            error!("Failed to publish guild_scheduled_event_update: {}", e);
        }
    }

    async fn guild_scheduled_event_delete(&self, ctx: Context, event: ScheduledEvent) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(event.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_guild_scheduled_event_delete(&event).await {
            error!("Failed to publish guild_scheduled_event_delete: {}", e);
        }
    }

    async fn channel_pins_update(&self, ctx: Context, pin: ChannelPinsUpdateEvent) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        // Only publish for guilds we have access to; skip DM pins
        if let Some(gid) = pin.guild_id {
            if !bridge.check_guild_access(gid.get()) {
                return;
            }
        }
        if let Err(e) = bridge.publish_channel_pins_update(&pin).await {
            error!("Failed to publish channel_pins_update: {}", e);
        }
    }

    async fn reaction_remove_all(
        &self,
        ctx: Context,
        channel_id: ChannelId,
        removed_from_message_id: MessageId,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Err(e) = bridge
            .publish_reaction_remove_all(channel_id, removed_from_message_id)
            .await
        {
            error!("Failed to publish reaction_remove_all: {}", e);
        }
    }

    async fn webhook_update(
        &self,
        ctx: Context,
        guild_id: GuildId,
        belongs_to_channel_id: ChannelId,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_webhooks_update(guild_id, belongs_to_channel_id).await {
            error!("Failed to publish webhooks_update: {}", e);
        }
    }

    async fn guild_stickers_update(
        &self,
        ctx: Context,
        guild_id: GuildId,
        current_state: HashMap<serenity::model::id::StickerId, serenity::model::sticker::Sticker>,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_guild_stickers_update(guild_id, &current_state).await {
            error!("Failed to publish guild_stickers_update: {}", e);
        }
    }

    async fn guild_integrations_update(&self, ctx: Context, guild_id: GuildId) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_guild_integrations_update(guild_id).await {
            error!("Failed to publish guild_integrations_update: {}", e);
        }
    }

    async fn guild_scheduled_event_user_add(
        &self,
        ctx: Context,
        subscribed: GuildScheduledEventUserAddEvent,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(subscribed.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_guild_scheduled_event_user_add(&subscribed).await {
            error!("Failed to publish scheduled_event_user_add: {}", e);
        }
    }

    async fn guild_scheduled_event_user_remove(
        &self,
        ctx: Context,
        unsubscribed: GuildScheduledEventUserRemoveEvent,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(unsubscribed.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_guild_scheduled_event_user_remove(&unsubscribed).await {
            error!("Failed to publish scheduled_event_user_remove: {}", e);
        }
    }

    async fn category_create(&self, ctx: Context, category: GuildChannel) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(category.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_category_create(&category).await {
            error!("Failed to publish category_create: {}", e);
        }
    }

    async fn category_delete(&self, ctx: Context, category: GuildChannel) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(category.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_category_delete(&category).await {
            error!("Failed to publish category_delete: {}", e);
        }
    }

    async fn resume(&self, ctx: Context, _event: serenity::model::event::ResumedEvent) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Err(e) = bridge.publish_bot_resume().await {
            error!("Failed to publish bot_resume: {}", e);
        }
    }

    async fn user_update(&self, ctx: Context, _old: Option<CurrentUser>, new: CurrentUser) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Err(e) = bridge.publish_user_update(&new).await {
            error!("Failed to publish user_update: {}", e);
        }
    }

    async fn guild_members_chunk(&self, ctx: Context, chunk: GuildMembersChunkEvent) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(chunk.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_guild_members_chunk(&chunk).await {
            error!("Failed to publish guild_members_chunk: {}", e);
        }
    }

    async fn voice_server_update(&self, ctx: Context, event: VoiceServerUpdateEvent) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Err(e) = bridge.publish_voice_server_update(&event).await {
            error!("Failed to publish voice_server_update: {}", e);
        }
    }

    async fn guild_audit_log_entry_create(
        &self,
        ctx: Context,
        entry: serenity::model::guild::audit_log::AuditLogEntry,
        guild_id: GuildId,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_guild_audit_log_entry(guild_id, &entry).await {
            error!("Failed to publish guild_audit_log_entry: {}", e);
        }
    }

    async fn thread_list_sync(&self, ctx: Context, thread_list_sync: ThreadListSyncEvent) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(thread_list_sync.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_thread_list_sync(&thread_list_sync).await {
            error!("Failed to publish thread_list_sync: {}", e);
        }
    }

    async fn thread_member_update(&self, ctx: Context, thread_member: ThreadMember) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Err(e) = bridge.publish_thread_member_update(&thread_member).await {
            error!("Failed to publish thread_member_update: {}", e);
        }
    }

    async fn integration_create(&self, ctx: Context, integration: Integration) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Err(e) = bridge.publish_integration_create(&integration).await {
            error!("Failed to publish integration_create: {}", e);
        }
    }

    async fn integration_update(&self, ctx: Context, integration: Integration) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Err(e) = bridge.publish_integration_update(&integration).await {
            error!("Failed to publish integration_update: {}", e);
        }
    }

    async fn integration_delete(
        &self,
        ctx: Context,
        integration_id: IntegrationId,
        guild_id: GuildId,
        application_id: Option<ApplicationId>,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_integration_delete(integration_id, guild_id, application_id).await {
            error!("Failed to publish integration_delete: {}", e);
        }
    }

    async fn voice_channel_status_update(
        &self,
        ctx: Context,
        old: Option<String>,
        status: Option<String>,
        id: ChannelId,
        guild_id: GuildId,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_voice_channel_status_update(old, status, id, guild_id).await {
            error!("Failed to publish voice_channel_status_update: {}", e);
        }
    }

    async fn auto_moderation_rule_create(&self, ctx: Context, rule: AutoModRule) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(rule.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_automod_rule_create(&rule).await {
            error!("Failed to publish automod_rule_create: {}", e);
        }
    }

    async fn auto_moderation_rule_update(&self, ctx: Context, rule: AutoModRule) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(rule.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_automod_rule_update(&rule).await {
            error!("Failed to publish automod_rule_update: {}", e);
        }
    }

    async fn auto_moderation_rule_delete(&self, ctx: Context, rule: AutoModRule) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(rule.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_automod_rule_delete(&rule).await {
            error!("Failed to publish automod_rule_delete: {}", e);
        }
    }

    async fn auto_moderation_action_execution(&self, ctx: Context, execution: ActionExecution) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(execution.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_automod_action_execution(&execution).await {
            error!("Failed to publish automod_action_execution: {}", e);
        }
    }

    async fn command_permissions_update(&self, ctx: Context, permission: CommandPermissions) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if !bridge.check_guild_access(permission.guild_id.get()) {
            return;
        }
        if let Err(e) = bridge.publish_command_permissions_update(&permission).await {
            error!("Failed to publish command_permissions_update: {}", e);
        }
    }

    async fn entitlement_create(&self, ctx: Context, entitlement: Entitlement) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Err(e) = bridge.publish_entitlement_create(&entitlement).await {
            error!("Failed to publish entitlement_create: {}", e);
        }
    }

    async fn entitlement_update(&self, ctx: Context, entitlement: Entitlement) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Err(e) = bridge.publish_entitlement_update(&entitlement).await {
            error!("Failed to publish entitlement_update: {}", e);
        }
    }

    async fn entitlement_delete(&self, ctx: Context, entitlement: Entitlement) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Err(e) = bridge.publish_entitlement_delete(&entitlement).await {
            error!("Failed to publish entitlement_delete: {}", e);
        }
    }

    async fn poll_vote_add(&self, ctx: Context, event: MessagePollVoteAddEvent) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Err(e) = bridge.publish_poll_vote_add(&event).await {
            error!("Failed to publish poll_vote_add: {}", e);
        }
    }

    async fn poll_vote_remove(&self, ctx: Context, event: MessagePollVoteRemoveEvent) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Err(e) = bridge.publish_poll_vote_remove(&event).await {
            error!("Failed to publish poll_vote_remove: {}", e);
        }
    }

    async fn reaction_remove_emoji(
        &self,
        ctx: Context,
        removed_reactions: serenity::model::channel::Reaction,
    ) {
        let bridge = {
            let data = ctx.data.read().await;
            match data.get::<DiscordBridge>() {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if let Err(e) = bridge.publish_reaction_remove_emoji(&removed_reactions).await {
            error!("Failed to publish reaction_remove_emoji: {}", e);
        }
    }
}
