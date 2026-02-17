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
//! - `telegram.{prefix}.bot.file.info`
//! - `telegram.{prefix}.bot.file.downloaded`
//! - `telegram.{prefix}.bot.payment.pre_checkout`
//! - `telegram.{prefix}.bot.payment.shipping`
//! - `telegram.{prefix}.bot.payment.successful`
//! - `telegram.{prefix}.bot.bot_commands.response`
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
//! - `telegram.{prefix}.agent.file.get`
//! - `telegram.{prefix}.agent.file.download`
//! - `telegram.{prefix}.agent.payment.send_invoice`
//! - `telegram.{prefix}.agent.payment.answer_pre_checkout`
//! - `telegram.{prefix}.agent.payment.answer_shipping`
//! - `telegram.{prefix}.agent.bot_commands.set`
//! - `telegram.{prefix}.agent.bot_commands.delete`
//! - `telegram.{prefix}.agent.bot_commands.get`

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

    /// Location message event subject
    pub fn message_location(prefix: &str) -> String {
        format!("telegram.{}.bot.message.location", prefix)
    }

    /// Venue message event subject
    pub fn message_venue(prefix: &str) -> String {
        format!("telegram.{}.bot.message.venue", prefix)
    }

    /// Contact message event subject
    pub fn message_contact(prefix: &str) -> String {
        format!("telegram.{}.bot.message.contact", prefix)
    }

    /// Sticker message event subject
    pub fn message_sticker(prefix: &str) -> String {
        format!("telegram.{}.bot.message.sticker", prefix)
    }

    /// Animation (GIF) message event subject
    pub fn message_animation(prefix: &str) -> String {
        format!("telegram.{}.bot.message.animation", prefix)
    }

    /// Video note message event subject
    pub fn message_video_note(prefix: &str) -> String {
        format!("telegram.{}.bot.message.video_note", prefix)
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

    /// Chat member updated event subject (other members)
    pub fn chat_member_updated(prefix: &str) -> String {
        format!("telegram.{}.bot.chat.member_updated", prefix)
    }

    /// My chat member updated event subject (bot itself)
    pub fn my_chat_member_updated(prefix: &str) -> String {
        format!("telegram.{}.bot.chat.my_member_updated", prefix)
    }

    /// File information response subject
    pub fn file_info(prefix: &str) -> String {
        format!("telegram.{}.bot.file.info", prefix)
    }

    /// File download response subject
    pub fn file_downloaded(prefix: &str) -> String {
        format!("telegram.{}.bot.file.downloaded", prefix)
    }

    /// Pre-checkout query event subject
    pub fn payment_pre_checkout(prefix: &str) -> String {
        format!("telegram.{}.bot.payment.pre_checkout", prefix)
    }

    /// Shipping query event subject
    pub fn payment_shipping(prefix: &str) -> String {
        format!("telegram.{}.bot.payment.shipping", prefix)
    }

    /// Successful payment event subject
    pub fn payment_successful(prefix: &str) -> String {
        format!("telegram.{}.bot.payment.successful", prefix)
    }

    /// Bot commands response subject
    pub fn bot_commands_response(prefix: &str) -> String {
        format!("telegram.{}.bot.bot_commands.response", prefix)
    }

    /// Sticker set info response subject
    pub fn sticker_set_info(prefix: &str) -> String {
        format!("telegram.{}.bot.sticker.set_info", prefix)
    }

    /// Uploaded sticker file response subject
    pub fn sticker_uploaded(prefix: &str) -> String {
        format!("telegram.{}.bot.sticker.uploaded", prefix)
    }

    /// Poll message event subject (poll sent inside a chat message)
    pub fn message_poll(prefix: &str) -> String {
        format!("telegram.{}.bot.message.poll", prefix)
    }

    /// Standalone poll update subject (poll state changed)
    pub fn poll_update(prefix: &str) -> String {
        format!("telegram.{}.bot.poll.update", prefix)
    }

    /// Poll answer event subject (user voted)
    pub fn poll_answer(prefix: &str) -> String {
        format!("telegram.{}.bot.poll.answer", prefix)
    }

    /// Error event subject for failed agent commands
    pub fn command_error(prefix: &str) -> String {
        format!("telegram.{}.bot.error.command", prefix)
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

    /// Send poll command subject
    pub fn poll_send(prefix: &str) -> String {
        format!("telegram.{}.agent.poll.send", prefix)
    }

    /// Stop poll command subject
    pub fn poll_stop(prefix: &str) -> String {
        format!("telegram.{}.agent.poll.stop", prefix)
    }

    /// Send video command subject
    pub fn message_send_video(prefix: &str) -> String {
        format!("telegram.{}.agent.message.send_video", prefix)
    }

    /// Send audio command subject
    pub fn message_send_audio(prefix: &str) -> String {
        format!("telegram.{}.agent.message.send_audio", prefix)
    }

    /// Send document command subject
    pub fn message_send_document(prefix: &str) -> String {
        format!("telegram.{}.agent.message.send_document", prefix)
    }

    /// Send voice command subject
    pub fn message_send_voice(prefix: &str) -> String {
        format!("telegram.{}.agent.message.send_voice", prefix)
    }

    /// Send media group command subject
    pub fn message_send_media_group(prefix: &str) -> String {
        format!("telegram.{}.agent.message.send_media_group", prefix)
    }

    /// Send location command subject
    pub fn message_send_location(prefix: &str) -> String {
        format!("telegram.{}.agent.message.send_location", prefix)
    }

    /// Send venue command subject
    pub fn message_send_venue(prefix: &str) -> String {
        format!("telegram.{}.agent.message.send_venue", prefix)
    }

    /// Send contact command subject
    pub fn message_send_contact(prefix: &str) -> String {
        format!("telegram.{}.agent.message.send_contact", prefix)
    }

    /// Send sticker command subject
    pub fn message_send_sticker(prefix: &str) -> String {
        format!("telegram.{}.agent.message.send_sticker", prefix)
    }

    /// Send animation (GIF) command subject
    pub fn message_send_animation(prefix: &str) -> String {
        format!("telegram.{}.agent.message.send_animation", prefix)
    }

    /// Send video note command subject
    pub fn message_send_video_note(prefix: &str) -> String {
        format!("telegram.{}.agent.message.send_video_note", prefix)
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

    /// Create forum topic command subject
    pub fn forum_create(prefix: &str) -> String {
        format!("telegram.{}.agent.forum.create", prefix)
    }

    /// Edit forum topic command subject
    pub fn forum_edit(prefix: &str) -> String {
        format!("telegram.{}.agent.forum.edit", prefix)
    }

    /// Close forum topic command subject
    pub fn forum_close(prefix: &str) -> String {
        format!("telegram.{}.agent.forum.close", prefix)
    }

    /// Reopen forum topic command subject
    pub fn forum_reopen(prefix: &str) -> String {
        format!("telegram.{}.agent.forum.reopen", prefix)
    }

    /// Delete forum topic command subject
    pub fn forum_delete(prefix: &str) -> String {
        format!("telegram.{}.agent.forum.delete", prefix)
    }

    /// Unpin all forum topic messages command subject
    pub fn forum_unpin(prefix: &str) -> String {
        format!("telegram.{}.agent.forum.unpin", prefix)
    }

    /// Edit general forum topic command subject
    pub fn forum_edit_general(prefix: &str) -> String {
        format!("telegram.{}.agent.forum.edit_general", prefix)
    }

    /// Close general forum topic command subject
    pub fn forum_close_general(prefix: &str) -> String {
        format!("telegram.{}.agent.forum.close_general", prefix)
    }

    /// Reopen general forum topic command subject
    pub fn forum_reopen_general(prefix: &str) -> String {
        format!("telegram.{}.agent.forum.reopen_general", prefix)
    }

    /// Hide general forum topic command subject
    pub fn forum_hide_general(prefix: &str) -> String {
        format!("telegram.{}.agent.forum.hide_general", prefix)
    }

    /// Unhide general forum topic command subject
    pub fn forum_unhide_general(prefix: &str) -> String {
        format!("telegram.{}.agent.forum.unhide_general", prefix)
    }

    /// Unpin all general forum topic messages command subject
    pub fn forum_unpin_general(prefix: &str) -> String {
        format!("telegram.{}.agent.forum.unpin_general", prefix)
    }

    /// Get file information command subject
    pub fn file_get(prefix: &str) -> String {
        format!("telegram.{}.agent.file.get", prefix)
    }

    /// Download file command subject
    pub fn file_download(prefix: &str) -> String {
        format!("telegram.{}.agent.file.download", prefix)
    }

    /// Promote chat member command subject
    pub fn admin_promote(prefix: &str) -> String {
        format!("telegram.{}.agent.admin.promote", prefix)
    }

    /// Restrict chat member command subject
    pub fn admin_restrict(prefix: &str) -> String {
        format!("telegram.{}.agent.admin.restrict", prefix)
    }

    /// Ban chat member command subject
    pub fn admin_ban(prefix: &str) -> String {
        format!("telegram.{}.agent.admin.ban", prefix)
    }

    /// Unban chat member command subject
    pub fn admin_unban(prefix: &str) -> String {
        format!("telegram.{}.agent.admin.unban", prefix)
    }

    /// Set chat permissions command subject
    pub fn admin_set_permissions(prefix: &str) -> String {
        format!("telegram.{}.agent.admin.set_permissions", prefix)
    }

    /// Set administrator custom title command subject
    pub fn admin_set_title(prefix: &str) -> String {
        format!("telegram.{}.agent.admin.set_title", prefix)
    }

    /// Pin message command subject
    pub fn admin_pin(prefix: &str) -> String {
        format!("telegram.{}.agent.admin.pin", prefix)
    }

    /// Unpin message command subject
    pub fn admin_unpin(prefix: &str) -> String {
        format!("telegram.{}.agent.admin.unpin", prefix)
    }

    /// Unpin all messages command subject
    pub fn admin_unpin_all(prefix: &str) -> String {
        format!("telegram.{}.agent.admin.unpin_all", prefix)
    }

    /// Set chat title command subject
    pub fn admin_set_chat_title(prefix: &str) -> String {
        format!("telegram.{}.agent.admin.set_title_chat", prefix)
    }

    /// Set chat description command subject
    pub fn admin_set_chat_description(prefix: &str) -> String {
        format!("telegram.{}.agent.admin.set_description", prefix)
    }

    /// Send invoice command subject
    pub fn payment_send_invoice(prefix: &str) -> String {
        format!("telegram.{}.agent.payment.send_invoice", prefix)
    }

    /// Answer pre-checkout query command subject
    pub fn payment_answer_pre_checkout(prefix: &str) -> String {
        format!("telegram.{}.agent.payment.answer_pre_checkout", prefix)
    }

    /// Answer shipping query command subject
    pub fn payment_answer_shipping(prefix: &str) -> String {
        format!("telegram.{}.agent.payment.answer_shipping", prefix)
    }

    /// Set bot commands command subject
    pub fn bot_commands_set(prefix: &str) -> String {
        format!("telegram.{}.agent.bot_commands.set", prefix)
    }

    /// Delete bot commands command subject
    pub fn bot_commands_delete(prefix: &str) -> String {
        format!("telegram.{}.agent.bot_commands.delete", prefix)
    }

    /// Get bot commands command subject
    pub fn bot_commands_get(prefix: &str) -> String {
        format!("telegram.{}.agent.bot_commands.get", prefix)
    }

    /// Get sticker set command subject
    pub fn sticker_get_set(prefix: &str) -> String {
        format!("telegram.{}.agent.sticker.get_set", prefix)
    }

    /// Upload sticker file command subject
    pub fn sticker_upload_file(prefix: &str) -> String {
        format!("telegram.{}.agent.sticker.upload_file", prefix)
    }

    /// Create new sticker set command subject
    pub fn sticker_create_set(prefix: &str) -> String {
        format!("telegram.{}.agent.sticker.create_set", prefix)
    }

    /// Add sticker to set command subject
    pub fn sticker_add_to_set(prefix: &str) -> String {
        format!("telegram.{}.agent.sticker.add_to_set", prefix)
    }

    /// Set sticker position in set command subject
    pub fn sticker_set_position(prefix: &str) -> String {
        format!("telegram.{}.agent.sticker.set_position", prefix)
    }

    /// Delete sticker from set command subject
    pub fn sticker_delete_from_set(prefix: &str) -> String {
        format!("telegram.{}.agent.sticker.delete_from_set", prefix)
    }

    /// Set sticker set title command subject
    pub fn sticker_set_title(prefix: &str) -> String {
        format!("telegram.{}.agent.sticker.set_title", prefix)
    }

    /// Set sticker set thumbnail command subject
    pub fn sticker_set_thumbnail(prefix: &str) -> String {
        format!("telegram.{}.agent.sticker.set_thumbnail", prefix)
    }

    /// Delete sticker set command subject
    pub fn sticker_delete_set(prefix: &str) -> String {
        format!("telegram.{}.agent.sticker.delete_set", prefix)
    }

    /// Set sticker emoji list command subject
    pub fn sticker_set_emoji_list(prefix: &str) -> String {
        format!("telegram.{}.agent.sticker.set_emoji_list", prefix)
    }

    /// Set sticker keywords command subject
    pub fn sticker_set_keywords(prefix: &str) -> String {
        format!("telegram.{}.agent.sticker.set_keywords", prefix)
    }

    /// Set sticker mask position command subject
    pub fn sticker_set_mask_position(prefix: &str) -> String {
        format!("telegram.{}.agent.sticker.set_mask_position", prefix)
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

    // ── New bot subjects ──────────────────────────────────────────────────────

    #[test]
    fn test_bot_poll_subjects() {
        assert_eq!(bot::message_poll("prod"), "telegram.prod.bot.message.poll");
        assert_eq!(bot::poll_update("prod"), "telegram.prod.bot.poll.update");
        assert_eq!(bot::poll_answer("prod"), "telegram.prod.bot.poll.answer");
    }

    #[test]
    fn test_bot_media_subjects() {
        assert_eq!(bot::message_video("prod"), "telegram.prod.bot.message.video");
        assert_eq!(bot::message_audio("prod"), "telegram.prod.bot.message.audio");
        assert_eq!(bot::message_document("prod"), "telegram.prod.bot.message.document");
        assert_eq!(bot::message_voice("prod"), "telegram.prod.bot.message.voice");
        assert_eq!(bot::message_sticker("prod"), "telegram.prod.bot.message.sticker");
        assert_eq!(bot::message_animation("prod"), "telegram.prod.bot.message.animation");
        assert_eq!(bot::message_video_note("prod"), "telegram.prod.bot.message.video_note");
    }

    #[test]
    fn test_bot_location_subjects() {
        assert_eq!(bot::message_location("prod"), "telegram.prod.bot.message.location");
        assert_eq!(bot::message_venue("prod"), "telegram.prod.bot.message.venue");
        assert_eq!(bot::message_contact("prod"), "telegram.prod.bot.message.contact");
    }

    #[test]
    fn test_bot_error_subject() {
        assert_eq!(bot::command_error("prod"), "telegram.prod.bot.error.command");
    }

    // ── New agent subjects ────────────────────────────────────────────────────

    #[test]
    fn test_agent_poll_subjects() {
        assert_eq!(agent::poll_send("prod"), "telegram.prod.agent.poll.send");
        assert_eq!(agent::poll_stop("prod"), "telegram.prod.agent.poll.stop");
    }

    #[test]
    fn test_agent_media_subjects() {
        assert_eq!(agent::message_send_video("prod"), "telegram.prod.agent.message.send_video");
        assert_eq!(agent::message_send_audio("prod"), "telegram.prod.agent.message.send_audio");
        assert_eq!(agent::message_send_document("prod"), "telegram.prod.agent.message.send_document");
        assert_eq!(agent::message_send_voice("prod"), "telegram.prod.agent.message.send_voice");
        assert_eq!(agent::message_send_media_group("prod"), "telegram.prod.agent.message.send_media_group");
    }

    #[test]
    fn test_agent_location_subjects() {
        assert_eq!(agent::message_send_location("prod"), "telegram.prod.agent.message.send_location");
        assert_eq!(agent::message_send_venue("prod"), "telegram.prod.agent.message.send_venue");
        assert_eq!(agent::message_send_contact("prod"), "telegram.prod.agent.message.send_contact");
    }

    #[test]
    fn test_agent_sticker_subjects() {
        assert_eq!(agent::message_send_sticker("prod"), "telegram.prod.agent.message.send_sticker");
        assert_eq!(agent::message_send_animation("prod"), "telegram.prod.agent.message.send_animation");
        assert_eq!(agent::message_send_video_note("prod"), "telegram.prod.agent.message.send_video_note");
        assert_eq!(agent::sticker_get_set("prod"), "telegram.prod.agent.sticker.get_set");
        assert_eq!(agent::sticker_create_set("prod"), "telegram.prod.agent.sticker.create_set");
    }

    #[test]
    fn test_prefix_isolation() {
        // Different prefixes must produce different subjects
        let s1 = bot::message_poll("env1");
        let s2 = bot::message_poll("env2");
        assert_ne!(s1, s2);
        assert!(s1.contains("env1"));
        assert!(s2.contains("env2"));
    }
}
