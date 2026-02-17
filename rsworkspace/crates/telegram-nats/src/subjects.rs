//! NATS subject patterns for Telegram integration
//!
//! Subject pattern: `telegram.{prefix}.{direction}.{entity}.{action}`
//!
//! Bot Events (Telegram → Agents):
//! - `telegram.{prefix}.bot.message.text`
//! - `telegram.{prefix}.bot.message.photo`
//! - `telegram.{prefix}.bot.message.video`
//! - `telegram.{prefix}.bot.message.audio`
//! - `telegram.{prefix}.bot.message.document`
//! - `telegram.{prefix}.bot.message.voice`
//! - `telegram.{prefix}.bot.callback.query`
//! - `telegram.{prefix}.bot.command.{command_name}`
//! - `telegram.{prefix}.bot.inline.query`
//! - `telegram.{prefix}.bot.inline.chosen`
//!
//! Agent Commands (Agents → Telegram):
//! - `telegram.{prefix}.agent.message.send`
//! - `telegram.{prefix}.agent.message.edit`
//! - `telegram.{prefix}.agent.message.delete`
//! - `telegram.{prefix}.agent.message.send_photo`
//! - `telegram.{prefix}.agent.message.stream`
//! - `telegram.{prefix}.agent.callback.answer`
//! - `telegram.{prefix}.agent.chat.action`
//! - `telegram.{prefix}.agent.inline.answer`

/// Subject builder for bot events
pub mod bot {
    /// Text message event subject
    pub fn message_text(prefix: &str) -> String {
        format!("telegram.{}.bot.message.text", prefix)
    }

    /// Photo message event subject
    pub fn message_photo(prefix: &str) -> String {
        format!("telegram.{}.bot.message.photo", prefix)
    }

    /// Video message event subject
    pub fn message_video(prefix: &str) -> String {
        format!("telegram.{}.bot.message.video", prefix)
    }

    /// Audio message event subject
    pub fn message_audio(prefix: &str) -> String {
        format!("telegram.{}.bot.message.audio", prefix)
    }

    /// Document message event subject
    pub fn message_document(prefix: &str) -> String {
        format!("telegram.{}.bot.message.document", prefix)
    }

    /// Voice message event subject
    pub fn message_voice(prefix: &str) -> String {
        format!("telegram.{}.bot.message.voice", prefix)
    }

    /// Callback query event subject
    pub fn callback_query(prefix: &str) -> String {
        format!("telegram.{}.bot.callback.query", prefix)
    }

    /// Command event subject
    pub fn command(prefix: &str, command_name: &str) -> String {
        format!("telegram.{}.bot.command.{}", prefix, command_name)
    }

    /// Wildcard subject for all bot events
    pub fn all(prefix: &str) -> String {
        format!("telegram.{}.bot.>", prefix)
    }

    /// Wildcard subject for all message events
    pub fn all_messages(prefix: &str) -> String {
        format!("telegram.{}.bot.message.>", prefix)
    }

    /// Wildcard subject for all commands
    pub fn all_commands(prefix: &str) -> String {
        format!("telegram.{}.bot.command.>", prefix)
    }

    /// Inline query event subject
    pub fn inline_query(prefix: &str) -> String {
        format!("telegram.{}.bot.inline.query", prefix)
    }

    /// Chosen inline result event subject
    pub fn chosen_inline_result(prefix: &str) -> String {
        format!("telegram.{}.bot.inline.chosen", prefix)
    }
}

/// Subject builder for agent commands
pub mod agent {
    /// Send message command subject
    pub fn message_send(prefix: &str) -> String {
        format!("telegram.{}.agent.message.send", prefix)
    }

    /// Edit message command subject
    pub fn message_edit(prefix: &str) -> String {
        format!("telegram.{}.agent.message.edit", prefix)
    }

    /// Delete message command subject
    pub fn message_delete(prefix: &str) -> String {
        format!("telegram.{}.agent.message.delete", prefix)
    }

    /// Send photo command subject
    pub fn message_send_photo(prefix: &str) -> String {
        format!("telegram.{}.agent.message.send_photo", prefix)
    }

    /// Stream message command subject
    pub fn message_stream(prefix: &str) -> String {
        format!("telegram.{}.agent.message.stream", prefix)
    }

    /// Answer callback command subject
    pub fn callback_answer(prefix: &str) -> String {
        format!("telegram.{}.agent.callback.answer", prefix)
    }

    /// Send chat action command subject
    pub fn chat_action(prefix: &str) -> String {
        format!("telegram.{}.agent.chat.action", prefix)
    }

    /// Answer inline query command subject
    pub fn inline_answer(prefix: &str) -> String {
        format!("telegram.{}.agent.inline.answer", prefix)
    }

    /// Wildcard subject for all agent commands
    pub fn all(prefix: &str) -> String {
        format!("telegram.{}.agent.>", prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bot_subjects() {
        assert_eq!(bot::message_text("prod"), "telegram.prod.bot.message.text");
        assert_eq!(bot::message_photo("dev"), "telegram.dev.bot.message.photo");
        assert_eq!(bot::command("prod", "start"), "telegram.prod.bot.command.start");
        assert_eq!(bot::all("prod"), "telegram.prod.bot.>");
    }

    #[test]
    fn test_agent_subjects() {
        assert_eq!(agent::message_send("prod"), "telegram.prod.agent.message.send");
        assert_eq!(agent::message_edit("dev"), "telegram.dev.agent.message.edit");
        assert_eq!(agent::all("prod"), "telegram.prod.agent.>");
    }
}
