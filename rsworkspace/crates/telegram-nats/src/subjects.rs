//! NATS subject patterns for Telegram integration
//!
//! Subject pattern: `telegram.{prefix}.{direction}.{entity}.{action}`
//!
//! Bot Events (Telegram → Agents):
//! - `tgbot.{prefix}.bot.message.text`
//! - `tgbot.{prefix}.bot.message.photo`
//! - `tgbot.{prefix}.bot.message.video`
//! - `tgbot.{prefix}.bot.message.audio`
//! - `tgbot.{prefix}.bot.message.document`
//! - `tgbot.{prefix}.bot.message.voice`
//! - `tgbot.{prefix}.bot.callback.query`
//! - `tgbot.{prefix}.bot.command.{command_name}`
//! - `tgbot.{prefix}.bot.inline.query`
//! - `tgbot.{prefix}.bot.inline.chosen`
//! - `tgbot.{prefix}.bot.file.info`
//! - `tgbot.{prefix}.bot.file.downloaded`
//! - `tgbot.{prefix}.bot.payment.pre_checkout`
//! - `tgbot.{prefix}.bot.payment.shipping`
//! - `tgbot.{prefix}.bot.payment.successful`
//! - `tgbot.{prefix}.bot.bot_commands.response`
//! - `tgbot.{prefix}.bot.forum.created`
//! - `tgbot.{prefix}.bot.forum.edited`
//! - `tgbot.{prefix}.bot.forum.closed`
//! - `tgbot.{prefix}.bot.forum.reopened`
//! - `tgbot.{prefix}.bot.forum.general_hidden`
//! - `tgbot.{prefix}.bot.forum.general_unhidden`
//! - `tgbot.{prefix}.bot.message.edited`
//! - `tgbot.{prefix}.bot.chat.join_request`
//!
//! Agent Commands (Agents → Telegram):
//! - `tgbot.{prefix}.agent.message.send`
//! - `tgbot.{prefix}.agent.message.edit`
//! - `tgbot.{prefix}.agent.message.delete`
//! - `tgbot.{prefix}.agent.message.forward`
//! - `tgbot.{prefix}.agent.message.copy`
//! - `tgbot.{prefix}.agent.message.send_photo`
//! - `tgbot.{prefix}.agent.message.stream`
//! - `tgbot.{prefix}.agent.callback.answer`
//! - `tgbot.{prefix}.agent.chat.action`
//! - `tgbot.{prefix}.agent.inline.answer`
//! - `tgbot.{prefix}.agent.file.get`
//! - `tgbot.{prefix}.agent.file.download`
//! - `tgbot.{prefix}.agent.payment.send_invoice`
//! - `tgbot.{prefix}.agent.payment.answer_pre_checkout`
//! - `tgbot.{prefix}.agent.payment.answer_shipping`
//! - `tgbot.{prefix}.agent.bot_commands.set`
//! - `tgbot.{prefix}.agent.bot_commands.delete`
//! - `tgbot.{prefix}.agent.bot_commands.get`

/// Subject builder for bot events
pub mod bot {
    /// Text message event subject
    pub fn message_text(prefix: &str) -> String {
        format!("tgbot.{}.bot.message.text", prefix)
    }

    /// Photo message event subject
    pub fn message_photo(prefix: &str) -> String {
        format!("tgbot.{}.bot.message.photo", prefix)
    }

    /// Video message event subject
    pub fn message_video(prefix: &str) -> String {
        format!("tgbot.{}.bot.message.video", prefix)
    }

    /// Audio message event subject
    pub fn message_audio(prefix: &str) -> String {
        format!("tgbot.{}.bot.message.audio", prefix)
    }

    /// Document message event subject
    pub fn message_document(prefix: &str) -> String {
        format!("tgbot.{}.bot.message.document", prefix)
    }

    /// Voice message event subject
    pub fn message_voice(prefix: &str) -> String {
        format!("tgbot.{}.bot.message.voice", prefix)
    }

    /// Location message event subject
    pub fn message_location(prefix: &str) -> String {
        format!("tgbot.{}.bot.message.location", prefix)
    }

    /// Venue message event subject
    pub fn message_venue(prefix: &str) -> String {
        format!("tgbot.{}.bot.message.venue", prefix)
    }

    /// Contact message event subject
    pub fn message_contact(prefix: &str) -> String {
        format!("tgbot.{}.bot.message.contact", prefix)
    }

    /// Sticker message event subject
    pub fn message_sticker(prefix: &str) -> String {
        format!("tgbot.{}.bot.message.sticker", prefix)
    }

    /// Animation (GIF) message event subject
    pub fn message_animation(prefix: &str) -> String {
        format!("tgbot.{}.bot.message.animation", prefix)
    }

    /// Video note message event subject
    pub fn message_video_note(prefix: &str) -> String {
        format!("tgbot.{}.bot.message.video_note", prefix)
    }

    /// Callback query event subject
    pub fn callback_query(prefix: &str) -> String {
        format!("tgbot.{}.bot.callback.query", prefix)
    }

    /// Command event subject
    pub fn command(prefix: &str, command_name: &str) -> String {
        format!("tgbot.{}.bot.command.{}", prefix, command_name)
    }

    /// Wildcard subject for all bot events
    pub fn all(prefix: &str) -> String {
        format!("tgbot.{}.bot.>", prefix)
    }

    /// Wildcard subject for all message events
    pub fn all_messages(prefix: &str) -> String {
        format!("tgbot.{}.bot.message.>", prefix)
    }

    /// Wildcard subject for all commands
    pub fn all_commands(prefix: &str) -> String {
        format!("tgbot.{}.bot.command.>", prefix)
    }

    /// Inline query event subject
    pub fn inline_query(prefix: &str) -> String {
        format!("tgbot.{}.bot.inline.query", prefix)
    }

    /// Chosen inline result event subject
    pub fn chosen_inline_result(prefix: &str) -> String {
        format!("tgbot.{}.bot.inline.chosen", prefix)
    }

    /// Chat member updated event subject (other members)
    pub fn chat_member_updated(prefix: &str) -> String {
        format!("tgbot.{}.bot.chat.member_updated", prefix)
    }

    /// My chat member updated event subject (bot itself)
    pub fn my_chat_member_updated(prefix: &str) -> String {
        format!("tgbot.{}.bot.chat.my_member_updated", prefix)
    }

    /// File information response subject
    pub fn file_info(prefix: &str) -> String {
        format!("tgbot.{}.bot.file.info", prefix)
    }

    /// File download response subject
    pub fn file_downloaded(prefix: &str) -> String {
        format!("tgbot.{}.bot.file.downloaded", prefix)
    }

    /// Pre-checkout query event subject
    pub fn payment_pre_checkout(prefix: &str) -> String {
        format!("tgbot.{}.bot.payment.pre_checkout", prefix)
    }

    /// Shipping query event subject
    pub fn payment_shipping(prefix: &str) -> String {
        format!("tgbot.{}.bot.payment.shipping", prefix)
    }

    /// Successful payment event subject
    pub fn payment_successful(prefix: &str) -> String {
        format!("tgbot.{}.bot.payment.successful", prefix)
    }

    /// Bot commands response subject
    pub fn bot_commands_response(prefix: &str) -> String {
        format!("tgbot.{}.bot.bot_commands.response", prefix)
    }

    /// Sticker set info response subject
    pub fn sticker_set_info(prefix: &str) -> String {
        format!("tgbot.{}.bot.sticker.set_info", prefix)
    }

    /// Uploaded sticker file response subject
    pub fn sticker_uploaded(prefix: &str) -> String {
        format!("tgbot.{}.bot.sticker.uploaded", prefix)
    }

    /// Poll message event subject (poll sent inside a chat message)
    pub fn message_poll(prefix: &str) -> String {
        format!("tgbot.{}.bot.message.poll", prefix)
    }

    /// Standalone poll update subject (poll state changed)
    pub fn poll_update(prefix: &str) -> String {
        format!("tgbot.{}.bot.poll.update", prefix)
    }

    /// Poll answer event subject (user voted)
    pub fn poll_answer(prefix: &str) -> String {
        format!("tgbot.{}.bot.poll.answer", prefix)
    }

    /// Error event subject for failed agent commands
    pub fn command_error(prefix: &str) -> String {
        format!("tgbot.{}.bot.error.command", prefix)
    }

    /// Forum topic created event subject
    pub fn forum_topic_created(prefix: &str) -> String {
        format!("tgbot.{}.bot.forum.created", prefix)
    }

    /// Forum topic edited event subject
    pub fn forum_topic_edited(prefix: &str) -> String {
        format!("tgbot.{}.bot.forum.edited", prefix)
    }

    /// Forum topic closed event subject
    pub fn forum_topic_closed(prefix: &str) -> String {
        format!("tgbot.{}.bot.forum.closed", prefix)
    }

    /// Forum topic reopened event subject
    pub fn forum_topic_reopened(prefix: &str) -> String {
        format!("tgbot.{}.bot.forum.reopened", prefix)
    }

    /// General forum topic hidden event subject
    pub fn general_forum_topic_hidden(prefix: &str) -> String {
        format!("tgbot.{}.bot.forum.general_hidden", prefix)
    }

    /// General forum topic unhidden event subject
    pub fn general_forum_topic_unhidden(prefix: &str) -> String {
        format!("tgbot.{}.bot.forum.general_unhidden", prefix)
    }

    /// Edited message event subject
    pub fn message_edited(prefix: &str) -> String {
        format!("tgbot.{}.bot.message.edited", prefix)
    }

    /// Chat join request event subject
    pub fn chat_join_request(prefix: &str) -> String {
        format!("tgbot.{}.bot.chat.join_request", prefix)
    }
}

/// Subject builder for agent commands
pub mod agent {
    /// Send message command subject
    pub fn message_send(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.send", prefix)
    }

    /// Edit message command subject
    pub fn message_edit(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.edit", prefix)
    }

    /// Delete message command subject
    pub fn message_delete(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.delete", prefix)
    }

    /// Forward message command subject
    pub fn message_forward(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.forward", prefix)
    }

    /// Copy message command subject
    pub fn message_copy(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.copy", prefix)
    }

    /// Send photo command subject
    pub fn message_send_photo(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.send_photo", prefix)
    }

    /// Send poll command subject
    pub fn poll_send(prefix: &str) -> String {
        format!("tgbot.{}.agent.poll.send", prefix)
    }

    /// Stop poll command subject
    pub fn poll_stop(prefix: &str) -> String {
        format!("tgbot.{}.agent.poll.stop", prefix)
    }

    /// Send video command subject
    pub fn message_send_video(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.send_video", prefix)
    }

    /// Send audio command subject
    pub fn message_send_audio(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.send_audio", prefix)
    }

    /// Send document command subject
    pub fn message_send_document(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.send_document", prefix)
    }

    /// Send voice command subject
    pub fn message_send_voice(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.send_voice", prefix)
    }

    /// Send media group command subject
    pub fn message_send_media_group(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.send_media_group", prefix)
    }

    /// Send location command subject
    pub fn message_send_location(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.send_location", prefix)
    }

    /// Send venue command subject
    pub fn message_send_venue(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.send_venue", prefix)
    }

    /// Send contact command subject
    pub fn message_send_contact(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.send_contact", prefix)
    }

    /// Send sticker command subject
    pub fn message_send_sticker(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.send_sticker", prefix)
    }

    /// Send animation (GIF) command subject
    pub fn message_send_animation(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.send_animation", prefix)
    }

    /// Send video note command subject
    pub fn message_send_video_note(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.send_video_note", prefix)
    }

    /// Stream message command subject
    pub fn message_stream(prefix: &str) -> String {
        format!("tgbot.{}.agent.message.stream", prefix)
    }

    /// Answer callback command subject
    pub fn callback_answer(prefix: &str) -> String {
        format!("tgbot.{}.agent.callback.answer", prefix)
    }

    /// Send chat action command subject
    pub fn chat_action(prefix: &str) -> String {
        format!("tgbot.{}.agent.chat.action", prefix)
    }

    /// Answer inline query command subject
    pub fn inline_answer(prefix: &str) -> String {
        format!("tgbot.{}.agent.inline.answer", prefix)
    }

    /// Create forum topic command subject
    pub fn forum_create(prefix: &str) -> String {
        format!("tgbot.{}.agent.forum.create", prefix)
    }

    /// Edit forum topic command subject
    pub fn forum_edit(prefix: &str) -> String {
        format!("tgbot.{}.agent.forum.edit", prefix)
    }

    /// Close forum topic command subject
    pub fn forum_close(prefix: &str) -> String {
        format!("tgbot.{}.agent.forum.close", prefix)
    }

    /// Reopen forum topic command subject
    pub fn forum_reopen(prefix: &str) -> String {
        format!("tgbot.{}.agent.forum.reopen", prefix)
    }

    /// Delete forum topic command subject
    pub fn forum_delete(prefix: &str) -> String {
        format!("tgbot.{}.agent.forum.delete", prefix)
    }

    /// Unpin all forum topic messages command subject
    pub fn forum_unpin(prefix: &str) -> String {
        format!("tgbot.{}.agent.forum.unpin", prefix)
    }

    /// Edit general forum topic command subject
    pub fn forum_edit_general(prefix: &str) -> String {
        format!("tgbot.{}.agent.forum.edit_general", prefix)
    }

    /// Close general forum topic command subject
    pub fn forum_close_general(prefix: &str) -> String {
        format!("tgbot.{}.agent.forum.close_general", prefix)
    }

    /// Reopen general forum topic command subject
    pub fn forum_reopen_general(prefix: &str) -> String {
        format!("tgbot.{}.agent.forum.reopen_general", prefix)
    }

    /// Hide general forum topic command subject
    pub fn forum_hide_general(prefix: &str) -> String {
        format!("tgbot.{}.agent.forum.hide_general", prefix)
    }

    /// Unhide general forum topic command subject
    pub fn forum_unhide_general(prefix: &str) -> String {
        format!("tgbot.{}.agent.forum.unhide_general", prefix)
    }

    /// Unpin all general forum topic messages command subject
    pub fn forum_unpin_general(prefix: &str) -> String {
        format!("tgbot.{}.agent.forum.unpin_general", prefix)
    }

    /// Get file information command subject
    pub fn file_get(prefix: &str) -> String {
        format!("tgbot.{}.agent.file.get", prefix)
    }

    /// Download file command subject
    pub fn file_download(prefix: &str) -> String {
        format!("tgbot.{}.agent.file.download", prefix)
    }

    /// Promote chat member command subject
    pub fn admin_promote(prefix: &str) -> String {
        format!("tgbot.{}.agent.admin.promote", prefix)
    }

    /// Restrict chat member command subject
    pub fn admin_restrict(prefix: &str) -> String {
        format!("tgbot.{}.agent.admin.restrict", prefix)
    }

    /// Ban chat member command subject
    pub fn admin_ban(prefix: &str) -> String {
        format!("tgbot.{}.agent.admin.ban", prefix)
    }

    /// Unban chat member command subject
    pub fn admin_unban(prefix: &str) -> String {
        format!("tgbot.{}.agent.admin.unban", prefix)
    }

    /// Set chat permissions command subject
    pub fn admin_set_permissions(prefix: &str) -> String {
        format!("tgbot.{}.agent.admin.set_permissions", prefix)
    }

    /// Set administrator custom title command subject
    pub fn admin_set_title(prefix: &str) -> String {
        format!("tgbot.{}.agent.admin.set_title", prefix)
    }

    /// Pin message command subject
    pub fn admin_pin(prefix: &str) -> String {
        format!("tgbot.{}.agent.admin.pin", prefix)
    }

    /// Unpin message command subject
    pub fn admin_unpin(prefix: &str) -> String {
        format!("tgbot.{}.agent.admin.unpin", prefix)
    }

    /// Unpin all messages command subject
    pub fn admin_unpin_all(prefix: &str) -> String {
        format!("tgbot.{}.agent.admin.unpin_all", prefix)
    }

    /// Set chat title command subject
    pub fn admin_set_chat_title(prefix: &str) -> String {
        format!("tgbot.{}.agent.admin.set_title_chat", prefix)
    }

    /// Set chat description command subject
    pub fn admin_set_chat_description(prefix: &str) -> String {
        format!("tgbot.{}.agent.admin.set_description", prefix)
    }

    /// Send invoice command subject
    pub fn payment_send_invoice(prefix: &str) -> String {
        format!("tgbot.{}.agent.payment.send_invoice", prefix)
    }

    /// Answer pre-checkout query command subject
    pub fn payment_answer_pre_checkout(prefix: &str) -> String {
        format!("tgbot.{}.agent.payment.answer_pre_checkout", prefix)
    }

    /// Answer shipping query command subject
    pub fn payment_answer_shipping(prefix: &str) -> String {
        format!("tgbot.{}.agent.payment.answer_shipping", prefix)
    }

    /// Set bot commands command subject
    pub fn bot_commands_set(prefix: &str) -> String {
        format!("tgbot.{}.agent.bot_commands.set", prefix)
    }

    /// Delete bot commands command subject
    pub fn bot_commands_delete(prefix: &str) -> String {
        format!("tgbot.{}.agent.bot_commands.delete", prefix)
    }

    /// Get bot commands command subject
    pub fn bot_commands_get(prefix: &str) -> String {
        format!("tgbot.{}.agent.bot_commands.get", prefix)
    }

    /// Get sticker set command subject
    pub fn sticker_get_set(prefix: &str) -> String {
        format!("tgbot.{}.agent.sticker.get_set", prefix)
    }

    /// Upload sticker file command subject
    pub fn sticker_upload_file(prefix: &str) -> String {
        format!("tgbot.{}.agent.sticker.upload_file", prefix)
    }

    /// Create new sticker set command subject
    pub fn sticker_create_set(prefix: &str) -> String {
        format!("tgbot.{}.agent.sticker.create_set", prefix)
    }

    /// Add sticker to set command subject
    pub fn sticker_add_to_set(prefix: &str) -> String {
        format!("tgbot.{}.agent.sticker.add_to_set", prefix)
    }

    /// Set sticker position in set command subject
    pub fn sticker_set_position(prefix: &str) -> String {
        format!("tgbot.{}.agent.sticker.set_position", prefix)
    }

    /// Delete sticker from set command subject
    pub fn sticker_delete_from_set(prefix: &str) -> String {
        format!("tgbot.{}.agent.sticker.delete_from_set", prefix)
    }

    /// Set sticker set title command subject
    pub fn sticker_set_title(prefix: &str) -> String {
        format!("tgbot.{}.agent.sticker.set_title", prefix)
    }

    /// Set sticker set thumbnail command subject
    pub fn sticker_set_thumbnail(prefix: &str) -> String {
        format!("tgbot.{}.agent.sticker.set_thumbnail", prefix)
    }

    /// Delete sticker set command subject
    pub fn sticker_delete_set(prefix: &str) -> String {
        format!("tgbot.{}.agent.sticker.delete_set", prefix)
    }

    /// Set sticker emoji list command subject
    pub fn sticker_set_emoji_list(prefix: &str) -> String {
        format!("tgbot.{}.agent.sticker.set_emoji_list", prefix)
    }

    /// Set sticker keywords command subject
    pub fn sticker_set_keywords(prefix: &str) -> String {
        format!("tgbot.{}.agent.sticker.set_keywords", prefix)
    }

    /// Set sticker mask position command subject
    pub fn sticker_set_mask_position(prefix: &str) -> String {
        format!("tgbot.{}.agent.sticker.set_mask_position", prefix)
    }

    /// Wildcard subject for all agent commands
    pub fn all(prefix: &str) -> String {
        format!("tgbot.{}.agent.>", prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bot_subjects() {
        assert_eq!(bot::message_text("prod"), "tgbot.prod.bot.message.text");
        assert_eq!(bot::message_photo("dev"), "tgbot.dev.bot.message.photo");
        assert_eq!(
            bot::command("prod", "start"),
            "tgbot.prod.bot.command.start"
        );
        assert_eq!(bot::all("prod"), "tgbot.prod.bot.>");
    }

    #[test]
    fn test_agent_subjects() {
        assert_eq!(
            agent::message_send("prod"),
            "tgbot.prod.agent.message.send"
        );
        assert_eq!(
            agent::message_edit("dev"),
            "tgbot.dev.agent.message.edit"
        );
        assert_eq!(agent::all("prod"), "tgbot.prod.agent.>");
    }

    // ── New bot subjects ──────────────────────────────────────────────────────

    #[test]
    fn test_bot_poll_subjects() {
        assert_eq!(bot::message_poll("prod"), "tgbot.prod.bot.message.poll");
        assert_eq!(bot::poll_update("prod"), "tgbot.prod.bot.poll.update");
        assert_eq!(bot::poll_answer("prod"), "tgbot.prod.bot.poll.answer");
    }

    #[test]
    fn test_bot_media_subjects() {
        assert_eq!(
            bot::message_video("prod"),
            "tgbot.prod.bot.message.video"
        );
        assert_eq!(
            bot::message_audio("prod"),
            "tgbot.prod.bot.message.audio"
        );
        assert_eq!(
            bot::message_document("prod"),
            "tgbot.prod.bot.message.document"
        );
        assert_eq!(
            bot::message_voice("prod"),
            "tgbot.prod.bot.message.voice"
        );
        assert_eq!(
            bot::message_sticker("prod"),
            "tgbot.prod.bot.message.sticker"
        );
        assert_eq!(
            bot::message_animation("prod"),
            "tgbot.prod.bot.message.animation"
        );
        assert_eq!(
            bot::message_video_note("prod"),
            "tgbot.prod.bot.message.video_note"
        );
    }

    #[test]
    fn test_bot_location_subjects() {
        assert_eq!(
            bot::message_location("prod"),
            "tgbot.prod.bot.message.location"
        );
        assert_eq!(
            bot::message_venue("prod"),
            "tgbot.prod.bot.message.venue"
        );
        assert_eq!(
            bot::message_contact("prod"),
            "tgbot.prod.bot.message.contact"
        );
    }

    #[test]
    fn test_bot_error_subject() {
        assert_eq!(
            bot::command_error("prod"),
            "tgbot.prod.bot.error.command"
        );
    }

    // ── New agent subjects ────────────────────────────────────────────────────

    #[test]
    fn test_agent_poll_subjects() {
        assert_eq!(agent::poll_send("prod"), "tgbot.prod.agent.poll.send");
        assert_eq!(agent::poll_stop("prod"), "tgbot.prod.agent.poll.stop");
    }

    #[test]
    fn test_agent_media_subjects() {
        assert_eq!(
            agent::message_send_video("prod"),
            "tgbot.prod.agent.message.send_video"
        );
        assert_eq!(
            agent::message_send_audio("prod"),
            "tgbot.prod.agent.message.send_audio"
        );
        assert_eq!(
            agent::message_send_document("prod"),
            "tgbot.prod.agent.message.send_document"
        );
        assert_eq!(
            agent::message_send_voice("prod"),
            "tgbot.prod.agent.message.send_voice"
        );
        assert_eq!(
            agent::message_send_media_group("prod"),
            "tgbot.prod.agent.message.send_media_group"
        );
    }

    #[test]
    fn test_agent_location_subjects() {
        assert_eq!(
            agent::message_send_location("prod"),
            "tgbot.prod.agent.message.send_location"
        );
        assert_eq!(
            agent::message_send_venue("prod"),
            "tgbot.prod.agent.message.send_venue"
        );
        assert_eq!(
            agent::message_send_contact("prod"),
            "tgbot.prod.agent.message.send_contact"
        );
    }

    #[test]
    fn test_agent_sticker_subjects() {
        assert_eq!(
            agent::message_send_sticker("prod"),
            "tgbot.prod.agent.message.send_sticker"
        );
        assert_eq!(
            agent::message_send_animation("prod"),
            "tgbot.prod.agent.message.send_animation"
        );
        assert_eq!(
            agent::message_send_video_note("prod"),
            "tgbot.prod.agent.message.send_video_note"
        );
        assert_eq!(
            agent::sticker_get_set("prod"),
            "tgbot.prod.agent.sticker.get_set"
        );
        assert_eq!(
            agent::sticker_create_set("prod"),
            "tgbot.prod.agent.sticker.create_set"
        );
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

    #[test]
    fn test_bot_callback_and_inline_subjects() {
        assert_eq!(
            bot::callback_query("prod"),
            "tgbot.prod.bot.callback.query"
        );
        assert_eq!(bot::inline_query("prod"), "tgbot.prod.bot.inline.query");
        assert_eq!(
            bot::chosen_inline_result("prod"),
            "tgbot.prod.bot.inline.chosen"
        );
    }

    #[test]
    fn test_bot_chat_member_subjects() {
        assert_eq!(
            bot::chat_member_updated("prod"),
            "tgbot.prod.bot.chat.member_updated"
        );
        assert_eq!(
            bot::my_chat_member_updated("prod"),
            "tgbot.prod.bot.chat.my_member_updated"
        );
    }

    #[test]
    fn test_bot_file_subjects() {
        assert_eq!(bot::file_info("prod"), "tgbot.prod.bot.file.info");
        assert_eq!(
            bot::file_downloaded("prod"),
            "tgbot.prod.bot.file.downloaded"
        );
    }

    #[test]
    fn test_bot_payment_subjects() {
        assert_eq!(
            bot::payment_pre_checkout("prod"),
            "tgbot.prod.bot.payment.pre_checkout"
        );
        assert_eq!(
            bot::payment_shipping("prod"),
            "tgbot.prod.bot.payment.shipping"
        );
        assert_eq!(
            bot::payment_successful("prod"),
            "tgbot.prod.bot.payment.successful"
        );
    }

    #[test]
    fn test_bot_bot_commands_and_sticker_subjects() {
        assert_eq!(
            bot::bot_commands_response("prod"),
            "tgbot.prod.bot.bot_commands.response"
        );
        assert_eq!(
            bot::sticker_set_info("prod"),
            "tgbot.prod.bot.sticker.set_info"
        );
        assert_eq!(
            bot::sticker_uploaded("prod"),
            "tgbot.prod.bot.sticker.uploaded"
        );
    }

    #[test]
    fn test_bot_wildcard_subjects() {
        assert_eq!(bot::all_messages("prod"), "tgbot.prod.bot.message.>");
        assert_eq!(bot::all_commands("prod"), "tgbot.prod.bot.command.>");
        assert_eq!(bot::all("prod"), "tgbot.prod.bot.>");
    }

    #[test]
    fn test_agent_streaming_callback_inline_subjects() {
        assert_eq!(
            agent::message_send_photo("prod"),
            "tgbot.prod.agent.message.send_photo"
        );
        assert_eq!(
            agent::message_stream("prod"),
            "tgbot.prod.agent.message.stream"
        );
        assert_eq!(
            agent::callback_answer("prod"),
            "tgbot.prod.agent.callback.answer"
        );
        assert_eq!(
            agent::chat_action("prod"),
            "tgbot.prod.agent.chat.action"
        );
        assert_eq!(
            agent::inline_answer("prod"),
            "tgbot.prod.agent.inline.answer"
        );
    }

    #[test]
    fn test_agent_file_subjects() {
        assert_eq!(agent::file_get("prod"), "tgbot.prod.agent.file.get");
        assert_eq!(
            agent::file_download("prod"),
            "tgbot.prod.agent.file.download"
        );
    }

    #[test]
    fn test_agent_admin_subjects() {
        assert_eq!(
            agent::admin_promote("prod"),
            "tgbot.prod.agent.admin.promote"
        );
        assert_eq!(
            agent::admin_restrict("prod"),
            "tgbot.prod.agent.admin.restrict"
        );
        assert_eq!(agent::admin_ban("prod"), "tgbot.prod.agent.admin.ban");
        assert_eq!(
            agent::admin_unban("prod"),
            "tgbot.prod.agent.admin.unban"
        );
        assert_eq!(
            agent::admin_set_permissions("prod"),
            "tgbot.prod.agent.admin.set_permissions"
        );
        assert_eq!(
            agent::admin_set_title("prod"),
            "tgbot.prod.agent.admin.set_title"
        );
        assert_eq!(agent::admin_pin("prod"), "tgbot.prod.agent.admin.pin");
        assert_eq!(
            agent::admin_unpin("prod"),
            "tgbot.prod.agent.admin.unpin"
        );
        assert_eq!(
            agent::admin_unpin_all("prod"),
            "tgbot.prod.agent.admin.unpin_all"
        );
        assert_eq!(
            agent::admin_set_chat_title("prod"),
            "tgbot.prod.agent.admin.set_title_chat"
        );
        assert_eq!(
            agent::admin_set_chat_description("prod"),
            "tgbot.prod.agent.admin.set_description"
        );
    }

    #[test]
    fn test_agent_payment_subjects() {
        assert_eq!(
            agent::payment_send_invoice("prod"),
            "tgbot.prod.agent.payment.send_invoice"
        );
        assert_eq!(
            agent::payment_answer_pre_checkout("prod"),
            "tgbot.prod.agent.payment.answer_pre_checkout"
        );
        assert_eq!(
            agent::payment_answer_shipping("prod"),
            "tgbot.prod.agent.payment.answer_shipping"
        );
    }

    #[test]
    fn test_agent_bot_commands_subjects() {
        assert_eq!(
            agent::bot_commands_set("prod"),
            "tgbot.prod.agent.bot_commands.set"
        );
        assert_eq!(
            agent::bot_commands_delete("prod"),
            "tgbot.prod.agent.bot_commands.delete"
        );
        assert_eq!(
            agent::bot_commands_get("prod"),
            "tgbot.prod.agent.bot_commands.get"
        );
    }

    #[test]
    fn test_agent_sticker_management_subjects() {
        assert_eq!(
            agent::sticker_upload_file("prod"),
            "tgbot.prod.agent.sticker.upload_file"
        );
        assert_eq!(
            agent::sticker_add_to_set("prod"),
            "tgbot.prod.agent.sticker.add_to_set"
        );
        assert_eq!(
            agent::sticker_set_position("prod"),
            "tgbot.prod.agent.sticker.set_position"
        );
        assert_eq!(
            agent::sticker_delete_from_set("prod"),
            "tgbot.prod.agent.sticker.delete_from_set"
        );
        assert_eq!(
            agent::sticker_set_title("prod"),
            "tgbot.prod.agent.sticker.set_title"
        );
        assert_eq!(
            agent::sticker_set_thumbnail("prod"),
            "tgbot.prod.agent.sticker.set_thumbnail"
        );
        assert_eq!(
            agent::sticker_delete_set("prod"),
            "tgbot.prod.agent.sticker.delete_set"
        );
        assert_eq!(
            agent::sticker_set_emoji_list("prod"),
            "tgbot.prod.agent.sticker.set_emoji_list"
        );
        assert_eq!(
            agent::sticker_set_keywords("prod"),
            "tgbot.prod.agent.sticker.set_keywords"
        );
        assert_eq!(
            agent::sticker_set_mask_position("prod"),
            "tgbot.prod.agent.sticker.set_mask_position"
        );
    }

    #[test]
    fn test_agent_forum_subjects() {
        assert_eq!(
            agent::forum_create("prod"),
            "tgbot.prod.agent.forum.create"
        );
        assert_eq!(agent::forum_edit("prod"), "tgbot.prod.agent.forum.edit");
        assert_eq!(
            agent::forum_close("prod"),
            "tgbot.prod.agent.forum.close"
        );
        assert_eq!(
            agent::forum_reopen("prod"),
            "tgbot.prod.agent.forum.reopen"
        );
        assert_eq!(
            agent::forum_delete("prod"),
            "tgbot.prod.agent.forum.delete"
        );
        assert_eq!(
            agent::forum_unpin("prod"),
            "tgbot.prod.agent.forum.unpin"
        );
        assert_eq!(
            agent::forum_edit_general("prod"),
            "tgbot.prod.agent.forum.edit_general"
        );
        assert_eq!(
            agent::forum_close_general("prod"),
            "tgbot.prod.agent.forum.close_general"
        );
        assert_eq!(
            agent::forum_reopen_general("prod"),
            "tgbot.prod.agent.forum.reopen_general"
        );
        assert_eq!(
            agent::forum_hide_general("prod"),
            "tgbot.prod.agent.forum.hide_general"
        );
        assert_eq!(
            agent::forum_unhide_general("prod"),
            "tgbot.prod.agent.forum.unhide_general"
        );
        assert_eq!(
            agent::forum_unpin_general("prod"),
            "tgbot.prod.agent.forum.unpin_general"
        );
    }

    #[test]
    fn test_agent_forward_copy_subjects() {
        assert_eq!(
            agent::message_forward("prod"),
            "tgbot.prod.agent.message.forward"
        );
        assert_eq!(
            agent::message_copy("prod"),
            "tgbot.prod.agent.message.copy"
        );
    }

    #[test]
    fn test_bot_edited_and_join_subjects() {
        assert_eq!(
            bot::message_edited("prod"),
            "tgbot.prod.bot.message.edited"
        );
        assert_eq!(
            bot::chat_join_request("prod"),
            "tgbot.prod.bot.chat.join_request"
        );
    }

    #[test]
    fn test_bot_forum_subjects() {
        assert_eq!(
            bot::forum_topic_created("prod"),
            "tgbot.prod.bot.forum.created"
        );
        assert_eq!(
            bot::forum_topic_edited("prod"),
            "tgbot.prod.bot.forum.edited"
        );
        assert_eq!(
            bot::forum_topic_closed("prod"),
            "tgbot.prod.bot.forum.closed"
        );
        assert_eq!(
            bot::forum_topic_reopened("prod"),
            "tgbot.prod.bot.forum.reopened"
        );
        assert_eq!(
            bot::general_forum_topic_hidden("prod"),
            "tgbot.prod.bot.forum.general_hidden"
        );
        assert_eq!(
            bot::general_forum_topic_unhidden("prod"),
            "tgbot.prod.bot.forum.general_unhidden"
        );
    }
}
