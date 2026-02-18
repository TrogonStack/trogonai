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
