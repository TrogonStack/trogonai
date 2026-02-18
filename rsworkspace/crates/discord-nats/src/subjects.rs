//! NATS subject patterns for Discord integration
//!
//! Subject pattern: `discord.{prefix}.{direction}.{entity}.{action}`
//!
//! Bot Events (Discord → Agents):
//! - `discord.{prefix}.bot.message.created`
//! - `discord.{prefix}.bot.message.updated`
//! - `discord.{prefix}.bot.message.deleted`
//! - `discord.{prefix}.bot.interaction.command`
//! - `discord.{prefix}.bot.interaction.component`
//! - `discord.{prefix}.bot.reaction.add`
//! - `discord.{prefix}.bot.reaction.remove`
//! - `discord.{prefix}.bot.guild.member_add`
//! - `discord.{prefix}.bot.guild.member_remove`
//!
//! Agent Commands (Agents → Discord):
//! - `discord.{prefix}.agent.message.send`
//! - `discord.{prefix}.agent.message.edit`
//! - `discord.{prefix}.agent.message.delete`
//! - `discord.{prefix}.agent.interaction.respond`
//! - `discord.{prefix}.agent.interaction.defer`
//! - `discord.{prefix}.agent.interaction.followup`
//! - `discord.{prefix}.agent.reaction.add`
//! - `discord.{prefix}.agent.reaction.remove`
//! - `discord.{prefix}.agent.channel.typing`

/// Subject builders for bot events (Discord → agents)
pub mod bot {
    /// Message created event subject
    pub fn message_created(prefix: &str) -> String {
        format!("discord.{}.bot.message.created", prefix)
    }

    /// Message updated event subject
    pub fn message_updated(prefix: &str) -> String {
        format!("discord.{}.bot.message.updated", prefix)
    }

    /// Message deleted event subject
    pub fn message_deleted(prefix: &str) -> String {
        format!("discord.{}.bot.message.deleted", prefix)
    }

    /// Slash command interaction event subject
    pub fn interaction_command(prefix: &str) -> String {
        format!("discord.{}.bot.interaction.command", prefix)
    }

    /// Component interaction event subject
    pub fn interaction_component(prefix: &str) -> String {
        format!("discord.{}.bot.interaction.component", prefix)
    }

    /// Reaction added event subject
    pub fn reaction_add(prefix: &str) -> String {
        format!("discord.{}.bot.reaction.add", prefix)
    }

    /// Reaction removed event subject
    pub fn reaction_remove(prefix: &str) -> String {
        format!("discord.{}.bot.reaction.remove", prefix)
    }

    /// Guild member added event subject
    pub fn guild_member_add(prefix: &str) -> String {
        format!("discord.{}.bot.guild.member_add", prefix)
    }

    /// Guild member removed event subject
    pub fn guild_member_remove(prefix: &str) -> String {
        format!("discord.{}.bot.guild.member_remove", prefix)
    }

    /// Message bulk delete event subject
    pub fn message_bulk_delete(prefix: &str) -> String {
        format!("discord.{}.bot.message.bulk_delete", prefix)
    }

    /// Guild member update event subject
    pub fn guild_member_update(prefix: &str) -> String {
        format!("discord.{}.bot.member.update", prefix)
    }

    /// Permanent Discord API command failures (published by the bot, consumed by agents/monitoring)
    pub fn command_error(prefix: &str) -> String {
        format!("discord.{}.bot.errors.command", prefix)
    }

    /// Typing start event subject
    pub fn typing_start(prefix: &str) -> String {
        format!("discord.{}.bot.typing.start", prefix)
    }

    /// Voice state update event subject
    pub fn voice_state_update(prefix: &str) -> String {
        format!("discord.{}.bot.voice.state_update", prefix)
    }

    /// Guild create event subject
    pub fn guild_create(prefix: &str) -> String {
        format!("discord.{}.bot.guild.create", prefix)
    }

    /// Guild update event subject
    pub fn guild_update(prefix: &str) -> String {
        format!("discord.{}.bot.guild.update", prefix)
    }

    /// Guild delete event subject
    pub fn guild_delete(prefix: &str) -> String {
        format!("discord.{}.bot.guild.delete", prefix)
    }

    /// Channel create event subject
    pub fn channel_create(prefix: &str) -> String {
        format!("discord.{}.bot.channel.create", prefix)
    }

    /// Channel update event subject
    pub fn channel_update(prefix: &str) -> String {
        format!("discord.{}.bot.channel.update", prefix)
    }

    /// Channel delete event subject
    pub fn channel_delete(prefix: &str) -> String {
        format!("discord.{}.bot.channel.delete", prefix)
    }

    /// Role create event subject
    pub fn role_create(prefix: &str) -> String {
        format!("discord.{}.bot.role.create", prefix)
    }

    /// Role update event subject
    pub fn role_update(prefix: &str) -> String {
        format!("discord.{}.bot.role.update", prefix)
    }

    /// Role delete event subject
    pub fn role_delete(prefix: &str) -> String {
        format!("discord.{}.bot.role.delete", prefix)
    }

    /// Presence update event subject
    pub fn presence_update(prefix: &str) -> String {
        format!("discord.{}.bot.presence.update", prefix)
    }

    /// Modal submit interaction event subject
    pub fn interaction_modal(prefix: &str) -> String {
        format!("discord.{}.bot.interaction.modal", prefix)
    }

    /// Autocomplete interaction event subject
    pub fn interaction_autocomplete(prefix: &str) -> String {
        format!("discord.{}.bot.interaction.autocomplete", prefix)
    }

    /// Bot ready event subject
    pub fn bot_ready(prefix: &str) -> String {
        format!("discord.{}.bot.ready", prefix)
    }

    /// Thread created event subject
    pub fn thread_created(prefix: &str) -> String {
        format!("discord.{}.bot.thread.created", prefix)
    }

    /// Thread updated event subject
    pub fn thread_updated(prefix: &str) -> String {
        format!("discord.{}.bot.thread.updated", prefix)
    }

    /// Thread deleted event subject
    pub fn thread_deleted(prefix: &str) -> String {
        format!("discord.{}.bot.thread.deleted", prefix)
    }

    /// Thread member(s) joined subject
    pub fn thread_member_add(prefix: &str) -> String {
        format!("discord.{}.bot.thread.member_add", prefix)
    }

    /// Thread member(s) left subject
    pub fn thread_member_remove(prefix: &str) -> String {
        format!("discord.{}.bot.thread.member_remove", prefix)
    }

    /// Invite created event subject
    pub fn invite_create(prefix: &str) -> String {
        format!("discord.{}.bot.invite.create", prefix)
    }

    /// Invite deleted event subject
    pub fn invite_delete(prefix: &str) -> String {
        format!("discord.{}.bot.invite.delete", prefix)
    }

    /// Stage instance created event subject
    pub fn stage_instance_create(prefix: &str) -> String {
        format!("discord.{}.bot.stage.instance_create", prefix)
    }

    /// Stage instance updated event subject
    pub fn stage_instance_update(prefix: &str) -> String {
        format!("discord.{}.bot.stage.instance_update", prefix)
    }

    /// Stage instance deleted event subject
    pub fn stage_instance_delete(prefix: &str) -> String {
        format!("discord.{}.bot.stage.instance_delete", prefix)
    }

    /// Guild ban add event subject
    pub fn guild_ban_add(prefix: &str) -> String {
        format!("discord.{}.bot.guild.ban_add", prefix)
    }

    /// Guild ban remove event subject
    pub fn guild_ban_remove(prefix: &str) -> String {
        format!("discord.{}.bot.guild.ban_remove", prefix)
    }

    /// Guild emojis update event subject
    pub fn guild_emojis_update(prefix: &str) -> String {
        format!("discord.{}.bot.guild.emojis_update", prefix)
    }

    /// Guild scheduled event created subject
    pub fn guild_scheduled_event_create(prefix: &str) -> String {
        format!("discord.{}.bot.scheduled_event.create", prefix)
    }

    /// Guild scheduled event updated subject
    pub fn guild_scheduled_event_update(prefix: &str) -> String {
        format!("discord.{}.bot.scheduled_event.update", prefix)
    }

    /// Guild scheduled event deleted subject
    pub fn guild_scheduled_event_delete(prefix: &str) -> String {
        format!("discord.{}.bot.scheduled_event.delete", prefix)
    }

    /// Channel pins updated event subject
    pub fn channel_pins_update(prefix: &str) -> String {
        format!("discord.{}.bot.channel.pins_update", prefix)
    }

    /// All reactions removed from message event subject
    pub fn reaction_remove_all_notification(prefix: &str) -> String {
        format!("discord.{}.bot.reaction.remove_all", prefix)
    }

    /// Webhooks updated event subject
    pub fn webhooks_update(prefix: &str) -> String {
        format!("discord.{}.bot.webhooks.update", prefix)
    }

    /// Guild stickers updated event subject
    pub fn guild_stickers_update(prefix: &str) -> String {
        format!("discord.{}.bot.guild.stickers_update", prefix)
    }

    /// Guild integrations updated event subject
    pub fn guild_integrations_update(prefix: &str) -> String {
        format!("discord.{}.bot.guild.integrations_update", prefix)
    }

    /// User subscribed to scheduled event subject
    pub fn scheduled_event_user_add(prefix: &str) -> String {
        format!("discord.{}.bot.scheduled_event.user_add", prefix)
    }

    /// User unsubscribed from scheduled event subject
    pub fn scheduled_event_user_remove(prefix: &str) -> String {
        format!("discord.{}.bot.scheduled_event.user_remove", prefix)
    }

    /// Wildcard for all bot events
    pub fn all(prefix: &str) -> String {
        format!("discord.{}.bot.>", prefix)
    }
}

/// Subject builders for agent commands (agents → Discord)
pub mod agent {
    /// Send message command subject
    pub fn message_send(prefix: &str) -> String {
        format!("discord.{}.agent.message.send", prefix)
    }

    /// Edit message command subject
    pub fn message_edit(prefix: &str) -> String {
        format!("discord.{}.agent.message.edit", prefix)
    }

    /// Delete message command subject
    pub fn message_delete(prefix: &str) -> String {
        format!("discord.{}.agent.message.delete", prefix)
    }

    /// Respond to interaction command subject
    pub fn interaction_respond(prefix: &str) -> String {
        format!("discord.{}.agent.interaction.respond", prefix)
    }

    /// Defer interaction command subject
    pub fn interaction_defer(prefix: &str) -> String {
        format!("discord.{}.agent.interaction.defer", prefix)
    }

    /// Interaction followup command subject
    pub fn interaction_followup(prefix: &str) -> String {
        format!("discord.{}.agent.interaction.followup", prefix)
    }

    /// Add reaction command subject
    pub fn reaction_add(prefix: &str) -> String {
        format!("discord.{}.agent.reaction.add", prefix)
    }

    /// Remove reaction command subject
    pub fn reaction_remove(prefix: &str) -> String {
        format!("discord.{}.agent.reaction.remove", prefix)
    }

    /// Broadcast typing indicator command subject
    pub fn channel_typing(prefix: &str) -> String {
        format!("discord.{}.agent.channel.typing", prefix)
    }

    /// Stream message command subject (progressive LLM responses)
    pub fn message_stream(prefix: &str) -> String {
        format!("discord.{}.agent.message.stream", prefix)
    }

    /// Respond to a modal interaction subject
    pub fn interaction_modal_respond(prefix: &str) -> String {
        format!("discord.{}.agent.interaction.modal_respond", prefix)
    }

    /// Respond to an autocomplete interaction subject
    pub fn interaction_autocomplete_respond(prefix: &str) -> String {
        format!("discord.{}.agent.interaction.autocomplete_respond", prefix)
    }

    /// Ban user command subject
    pub fn guild_ban(prefix: &str) -> String {
        format!("discord.{}.agent.guild.ban", prefix)
    }

    /// Kick user command subject
    pub fn guild_kick(prefix: &str) -> String {
        format!("discord.{}.agent.guild.kick", prefix)
    }

    /// Timeout user command subject
    pub fn guild_timeout(prefix: &str) -> String {
        format!("discord.{}.agent.guild.timeout", prefix)
    }

    /// Create channel command subject
    pub fn channel_create(prefix: &str) -> String {
        format!("discord.{}.agent.channel.create", prefix)
    }

    /// Edit channel command subject
    pub fn channel_edit(prefix: &str) -> String {
        format!("discord.{}.agent.channel.edit", prefix)
    }

    /// Delete channel command subject
    pub fn channel_delete(prefix: &str) -> String {
        format!("discord.{}.agent.channel.delete", prefix)
    }

    /// Create role command subject
    pub fn role_create(prefix: &str) -> String {
        format!("discord.{}.agent.role.create", prefix)
    }

    /// Assign role command subject
    pub fn role_assign(prefix: &str) -> String {
        format!("discord.{}.agent.role.assign", prefix)
    }

    /// Remove role command subject
    pub fn role_remove(prefix: &str) -> String {
        format!("discord.{}.agent.role.remove", prefix)
    }

    /// Delete role command subject
    pub fn role_delete(prefix: &str) -> String {
        format!("discord.{}.agent.role.delete", prefix)
    }

    /// Pin message command subject
    pub fn message_pin(prefix: &str) -> String {
        format!("discord.{}.agent.message.pin", prefix)
    }

    /// Unpin message command subject
    pub fn message_unpin(prefix: &str) -> String {
        format!("discord.{}.agent.message.unpin", prefix)
    }

    /// Bulk delete messages command subject
    pub fn message_bulk_delete(prefix: &str) -> String {
        format!("discord.{}.agent.message.bulk_delete", prefix)
    }

    /// Create thread command subject
    pub fn thread_create(prefix: &str) -> String {
        format!("discord.{}.agent.thread.create", prefix)
    }

    /// Archive thread command subject
    pub fn thread_archive(prefix: &str) -> String {
        format!("discord.{}.agent.thread.archive", prefix)
    }

    /// Set bot presence/status command subject
    pub fn bot_presence(prefix: &str) -> String {
        format!("discord.{}.agent.bot.presence", prefix)
    }

    /// Fetch messages from a channel (request-reply)
    pub fn fetch_messages(prefix: &str) -> String {
        format!("discord.{}.agent.fetch.messages", prefix)
    }

    /// Fetch a single guild member (request-reply)
    pub fn fetch_member(prefix: &str) -> String {
        format!("discord.{}.agent.fetch.member", prefix)
    }

    /// Unban user command subject
    pub fn guild_unban(prefix: &str) -> String {
        format!("discord.{}.agent.guild.unban", prefix)
    }

    /// Set guild member nickname command subject
    pub fn guild_member_nick(prefix: &str) -> String {
        format!("discord.{}.agent.member.nick", prefix)
    }

    /// Create webhook command subject
    pub fn webhook_create(prefix: &str) -> String {
        format!("discord.{}.agent.webhook.create", prefix)
    }

    /// Execute webhook command subject
    pub fn webhook_execute(prefix: &str) -> String {
        format!("discord.{}.agent.webhook.execute", prefix)
    }

    /// Delete webhook command subject
    pub fn webhook_delete(prefix: &str) -> String {
        format!("discord.{}.agent.webhook.delete", prefix)
    }

    /// Move user to voice channel command subject
    pub fn voice_move(prefix: &str) -> String {
        format!("discord.{}.agent.voice.move", prefix)
    }

    /// Disconnect user from voice channel command subject
    pub fn voice_disconnect(prefix: &str) -> String {
        format!("discord.{}.agent.voice.disconnect", prefix)
    }

    /// Create invite command subject
    pub fn invite_create(prefix: &str) -> String {
        format!("discord.{}.agent.invite.create", prefix)
    }

    /// Revoke invite command subject
    pub fn invite_revoke(prefix: &str) -> String {
        format!("discord.{}.agent.invite.revoke", prefix)
    }

    /// Create emoji command subject
    pub fn emoji_create(prefix: &str) -> String {
        format!("discord.{}.agent.emoji.create", prefix)
    }

    /// Delete emoji command subject
    pub fn emoji_delete(prefix: &str) -> String {
        format!("discord.{}.agent.emoji.delete", prefix)
    }

    /// Create scheduled event command subject
    pub fn scheduled_event_create(prefix: &str) -> String {
        format!("discord.{}.agent.scheduled_event.create", prefix)
    }

    /// Delete scheduled event command subject
    pub fn scheduled_event_delete(prefix: &str) -> String {
        format!("discord.{}.agent.scheduled_event.delete", prefix)
    }

    /// Remove all reactions command subject
    pub fn reaction_remove_all(prefix: &str) -> String {
        format!("discord.{}.agent.reaction.remove_all", prefix)
    }

    /// Crosspost message command subject
    pub fn message_crosspost(prefix: &str) -> String {
        format!("discord.{}.agent.message.crosspost", prefix)
    }

    /// Create forum post command subject
    pub fn forum_post_create(prefix: &str) -> String {
        format!("discord.{}.agent.forum.post_create", prefix)
    }

    /// Add member to thread command subject
    pub fn thread_member_add(prefix: &str) -> String {
        format!("discord.{}.agent.thread.member_add", prefix)
    }

    /// Remove member from thread command subject
    pub fn thread_member_remove(prefix: &str) -> String {
        format!("discord.{}.agent.thread.member_remove", prefix)
    }

    /// Edit role command subject
    pub fn role_edit(prefix: &str) -> String {
        format!("discord.{}.agent.role.edit", prefix)
    }

    /// Create stage instance command subject
    pub fn stage_create(prefix: &str) -> String {
        format!("discord.{}.agent.stage.create", prefix)
    }

    /// Delete stage instance command subject
    pub fn stage_delete(prefix: &str) -> String {
        format!("discord.{}.agent.stage.delete", prefix)
    }

    /// Edit webhook command subject
    pub fn webhook_edit(prefix: &str) -> String {
        format!("discord.{}.agent.webhook.edit", prefix)
    }

    /// Create DM channel command subject (request-reply)
    pub fn dm_create(prefix: &str) -> String {
        format!("discord.{}.agent.dm.create", prefix)
    }

    /// Fetch guild info subject (request-reply)
    pub fn fetch_guild(prefix: &str) -> String {
        format!("discord.{}.agent.fetch.guild", prefix)
    }

    /// Fetch channel info subject (request-reply)
    pub fn fetch_channel(prefix: &str) -> String {
        format!("discord.{}.agent.fetch.channel", prefix)
    }

    /// Fetch channel invites subject (request-reply)
    pub fn fetch_invites(prefix: &str) -> String {
        format!("discord.{}.agent.fetch.invites", prefix)
    }

    /// Edit scheduled event command subject
    pub fn scheduled_event_edit(prefix: &str) -> String {
        format!("discord.{}.agent.scheduled_event.edit", prefix)
    }

    /// Edit emoji command subject
    pub fn emoji_edit(prefix: &str) -> String {
        format!("discord.{}.agent.emoji.edit", prefix)
    }

    /// Fetch guild members subject (request-reply)
    pub fn fetch_guild_members(prefix: &str) -> String {
        format!("discord.{}.agent.fetch.guild_members", prefix)
    }

    /// Fetch guild channels subject (request-reply)
    pub fn fetch_guild_channels(prefix: &str) -> String {
        format!("discord.{}.agent.fetch.guild_channels", prefix)
    }

    /// Fetch pinned messages subject (request-reply)
    pub fn fetch_pinned(prefix: &str) -> String {
        format!("discord.{}.agent.fetch.pinned", prefix)
    }

    /// Fetch guild roles subject (request-reply)
    pub fn fetch_roles(prefix: &str) -> String {
        format!("discord.{}.agent.fetch.roles", prefix)
    }

    /// Edit original interaction response command subject
    pub fn interaction_edit_response(prefix: &str) -> String {
        format!("discord.{}.agent.interaction.edit_response", prefix)
    }

    /// Delete original interaction response command subject
    pub fn interaction_delete_response(prefix: &str) -> String {
        format!("discord.{}.agent.interaction.delete_response", prefix)
    }

    /// Set channel permission overwrite command subject
    pub fn channel_set_permissions(prefix: &str) -> String {
        format!("discord.{}.agent.channel.set_permissions", prefix)
    }

    /// Delete channel permission overwrite command subject
    pub fn channel_delete_permissions(prefix: &str) -> String {
        format!("discord.{}.agent.channel.delete_permissions", prefix)
    }

    /// Wildcard for all agent commands
    pub fn all(prefix: &str) -> String {
        format!("discord.{}.agent.>", prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bot_subjects() {
        assert_eq!(
            bot::message_created("prod"),
            "discord.prod.bot.message.created"
        );
        assert_eq!(
            bot::message_updated("prod"),
            "discord.prod.bot.message.updated"
        );
        assert_eq!(
            bot::message_deleted("prod"),
            "discord.prod.bot.message.deleted"
        );
        assert_eq!(
            bot::interaction_command("prod"),
            "discord.prod.bot.interaction.command"
        );
        assert_eq!(
            bot::interaction_component("prod"),
            "discord.prod.bot.interaction.component"
        );
        assert_eq!(bot::reaction_add("prod"), "discord.prod.bot.reaction.add");
        assert_eq!(
            bot::reaction_remove("prod"),
            "discord.prod.bot.reaction.remove"
        );
        assert_eq!(
            bot::guild_member_add("prod"),
            "discord.prod.bot.guild.member_add"
        );
        assert_eq!(
            bot::guild_member_remove("prod"),
            "discord.prod.bot.guild.member_remove"
        );
        assert_eq!(
            bot::command_error("prod"),
            "discord.prod.bot.errors.command"
        );
        assert_eq!(bot::all("prod"), "discord.prod.bot.>");
    }

    #[test]
    fn test_agent_subjects() {
        assert_eq!(
            agent::message_send("prod"),
            "discord.prod.agent.message.send"
        );
        assert_eq!(
            agent::message_edit("prod"),
            "discord.prod.agent.message.edit"
        );
        assert_eq!(
            agent::message_delete("prod"),
            "discord.prod.agent.message.delete"
        );
        assert_eq!(
            agent::interaction_respond("prod"),
            "discord.prod.agent.interaction.respond"
        );
        assert_eq!(
            agent::interaction_defer("prod"),
            "discord.prod.agent.interaction.defer"
        );
        assert_eq!(
            agent::interaction_followup("prod"),
            "discord.prod.agent.interaction.followup"
        );
        assert_eq!(
            agent::reaction_add("prod"),
            "discord.prod.agent.reaction.add"
        );
        assert_eq!(
            agent::reaction_remove("prod"),
            "discord.prod.agent.reaction.remove"
        );
        assert_eq!(
            agent::channel_typing("prod"),
            "discord.prod.agent.channel.typing"
        );
        assert_eq!(
            agent::message_stream("prod"),
            "discord.prod.agent.message.stream"
        );
        assert_eq!(agent::all("prod"), "discord.prod.agent.>");
    }

    #[test]
    fn test_bot_subjects_new() {
        assert_eq!(bot::typing_start("prod"),              "discord.prod.bot.typing.start");
        assert_eq!(bot::voice_state_update("prod"),        "discord.prod.bot.voice.state_update");
        assert_eq!(bot::guild_create("prod"),              "discord.prod.bot.guild.create");
        assert_eq!(bot::guild_update("prod"),              "discord.prod.bot.guild.update");
        assert_eq!(bot::guild_delete("prod"),              "discord.prod.bot.guild.delete");
        assert_eq!(bot::channel_create("prod"),            "discord.prod.bot.channel.create");
        assert_eq!(bot::channel_update("prod"),            "discord.prod.bot.channel.update");
        assert_eq!(bot::channel_delete("prod"),            "discord.prod.bot.channel.delete");
        assert_eq!(bot::role_create("prod"),               "discord.prod.bot.role.create");
        assert_eq!(bot::role_update("prod"),               "discord.prod.bot.role.update");
        assert_eq!(bot::role_delete("prod"),               "discord.prod.bot.role.delete");
        assert_eq!(bot::presence_update("prod"),           "discord.prod.bot.presence.update");
        assert_eq!(bot::interaction_modal("prod"),         "discord.prod.bot.interaction.modal");
        assert_eq!(bot::interaction_autocomplete("prod"),  "discord.prod.bot.interaction.autocomplete");
        assert_eq!(bot::bot_ready("prod"),                 "discord.prod.bot.ready");
    }

    #[test]
    fn test_agent_subjects_new() {
        assert_eq!(agent::interaction_modal_respond("prod"),       "discord.prod.agent.interaction.modal_respond");
        assert_eq!(agent::interaction_autocomplete_respond("prod"),"discord.prod.agent.interaction.autocomplete_respond");
        assert_eq!(agent::guild_ban("prod"),                       "discord.prod.agent.guild.ban");
        assert_eq!(agent::guild_kick("prod"),                      "discord.prod.agent.guild.kick");
        assert_eq!(agent::guild_timeout("prod"),                   "discord.prod.agent.guild.timeout");
        assert_eq!(agent::channel_create("prod"),                  "discord.prod.agent.channel.create");
        assert_eq!(agent::channel_edit("prod"),                    "discord.prod.agent.channel.edit");
        assert_eq!(agent::channel_delete("prod"),                  "discord.prod.agent.channel.delete");
        assert_eq!(agent::role_create("prod"),                     "discord.prod.agent.role.create");
        assert_eq!(agent::role_assign("prod"),                     "discord.prod.agent.role.assign");
        assert_eq!(agent::role_remove("prod"),                     "discord.prod.agent.role.remove");
        assert_eq!(agent::role_delete("prod"),                     "discord.prod.agent.role.delete");
        assert_eq!(agent::message_pin("prod"),                     "discord.prod.agent.message.pin");
        assert_eq!(agent::message_unpin("prod"),                   "discord.prod.agent.message.unpin");
        assert_eq!(agent::message_bulk_delete("prod"),             "discord.prod.agent.message.bulk_delete");
        assert_eq!(agent::thread_create("prod"),                   "discord.prod.agent.thread.create");
        assert_eq!(agent::thread_archive("prod"),                  "discord.prod.agent.thread.archive");
        assert_eq!(agent::bot_presence("prod"),                    "discord.prod.agent.bot.presence");
        assert_eq!(agent::fetch_messages("prod"),                  "discord.prod.agent.fetch.messages");
        assert_eq!(agent::fetch_member("prod"),                    "discord.prod.agent.fetch.member");
    }

    #[test]
    fn test_medium_impact_subjects() {
        // Bot events
        assert_eq!(bot::invite_create("prod"),  "discord.prod.bot.invite.create");
        assert_eq!(bot::invite_delete("prod"),  "discord.prod.bot.invite.delete");
        // Agent commands
        assert_eq!(agent::invite_create("prod"),           "discord.prod.agent.invite.create");
        assert_eq!(agent::invite_revoke("prod"),           "discord.prod.agent.invite.revoke");
        assert_eq!(agent::emoji_create("prod"),            "discord.prod.agent.emoji.create");
        assert_eq!(agent::emoji_delete("prod"),            "discord.prod.agent.emoji.delete");
        assert_eq!(agent::scheduled_event_create("prod"),  "discord.prod.agent.scheduled_event.create");
        assert_eq!(agent::scheduled_event_delete("prod"),  "discord.prod.agent.scheduled_event.delete");
    }

    #[test]
    fn test_thread_and_voice_subjects() {
        // Thread events (bot)
        assert_eq!(bot::thread_created("prod"),       "discord.prod.bot.thread.created");
        assert_eq!(bot::thread_updated("prod"),       "discord.prod.bot.thread.updated");
        assert_eq!(bot::thread_deleted("prod"),       "discord.prod.bot.thread.deleted");
        assert_eq!(bot::thread_member_add("prod"),    "discord.prod.bot.thread.member_add");
        assert_eq!(bot::thread_member_remove("prod"), "discord.prod.bot.thread.member_remove");
        // Webhook commands (agent)
        assert_eq!(agent::webhook_create("prod"),     "discord.prod.agent.webhook.create");
        assert_eq!(agent::webhook_execute("prod"),    "discord.prod.agent.webhook.execute");
        assert_eq!(agent::webhook_delete("prod"),     "discord.prod.agent.webhook.delete");
        // Voice commands (agent)
        assert_eq!(agent::voice_move("prod"),         "discord.prod.agent.voice.move");
        assert_eq!(agent::voice_disconnect("prod"),   "discord.prod.agent.voice.disconnect");
    }

    #[test]
    fn test_prefix_substitution() {
        assert_eq!(
            bot::message_created("dev"),
            "discord.dev.bot.message.created"
        );
        assert_eq!(
            agent::message_send("staging"),
            "discord.staging.agent.message.send"
        );
    }

    #[test]
    fn test_subjects_start_with_discord() {
        for s in [
            bot::message_created("prod"),
            bot::interaction_command("prod"),
            agent::message_send("prod"),
            agent::channel_typing("prod"),
        ] {
            assert!(
                s.starts_with("discord."),
                "Subject should start with 'discord.': {}",
                s
            );
        }
    }

    #[test]
    fn test_bot_subjects_contain_bot_direction() {
        for s in [
            bot::message_created("prod"),
            bot::reaction_add("prod"),
            bot::guild_member_add("prod"),
        ] {
            assert!(
                s.contains(".bot."),
                "Bot subject should contain '.bot.': {}",
                s
            );
        }
    }

    #[test]
    fn test_agent_subjects_contain_agent_direction() {
        for s in [
            agent::message_send("prod"),
            agent::interaction_respond("prod"),
            agent::channel_typing("prod"),
        ] {
            assert!(
                s.contains(".agent."),
                "Agent subject should contain '.agent.': {}",
                s
            );
        }
    }
}
