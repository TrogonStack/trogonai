//! Outbound message processor (NATS → Telegram)

#[cfg(test)]
#[path = "outbound_tests.rs"]
mod outbound_tests;

use crate::errors::{classify, ErrorOutcome};
use anyhow::Result;
use async_nats::Client;
use std::collections::HashMap;
use std::sync::Arc;
use telegram_nats::{subjects, MessageSubscriber};
use telegram_types::commands::*;
use teloxide::types::{ChatId, MessageId, ParseMode as TgParseMode};
use teloxide::{requests::Requester, Bot};
use tokio::sync::RwLock;
use tokio::time::Instant;
use tracing::{debug, error, info, warn};

/// Signals the dispatch loop how to NAK a JetStream message after a Telegram error.
#[derive(Debug)]
pub(crate) struct NakError {
    /// Delay before redelivery; `None` means redeliver immediately.
    pub delay: Option<std::time::Duration>,
}

impl std::fmt::Display for NakError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.delay {
            Some(d) => write!(f, "nak with delay {:?}", d),
            None => write!(f, "nak immediately"),
        }
    }
}

impl std::error::Error for NakError {}

/// Tracking info for streaming messages
#[derive(Debug, Clone)]
pub(crate) struct StreamingMessage {
    pub message_id: i32,
    pub last_edit: Instant,
    pub edit_count: u32,
}

/// Per-message context set before each dispatch call and read by `handle_telegram_error`.
///
/// Populated from NATS message headers (Gap B) and from a generic chat_id
/// parse of the payload.  Kept in an `Arc<RwLock<…>>` so `handle_telegram_error`
/// can read it from `&self` without changing the signature of every handler.
#[derive(Default, Clone)]
struct DispatchContext {
    session_id: Option<String>,
    chat_id: Option<i64>,
}

/// Outbound processor that listens to agent commands and executes them
pub struct OutboundProcessor {
    pub(crate) bot: Bot,
    pub(crate) subscriber: MessageSubscriber,
    pub(crate) js: async_nats::jetstream::Context,
    /// Track streaming messages by (chat_id, session_id)
    pub(crate) streaming_messages: Arc<RwLock<HashMap<(i64, String), StreamingMessage>>>,
    /// Dedup store — prevents duplicate Telegram API calls on JetStream redelivery
    dedup: Option<crate::dedup::DedupStore>,
    /// Context for the message currently being dispatched (session_id, chat_id)
    dispatch_ctx: Arc<RwLock<DispatchContext>>,
}

impl OutboundProcessor {
    /// Create a new outbound processor
    pub fn new(
        bot: Bot,
        client: Client,
        prefix: String,
        js: async_nats::jetstream::Context,
        dedup_kv: Option<async_nats::jetstream::kv::Store>,
    ) -> Self {
        Self {
            bot,
            subscriber: MessageSubscriber::new(client.clone(), prefix),
            js,
            streaming_messages: Arc::new(RwLock::new(HashMap::new())),
            dedup: dedup_kv.map(crate::dedup::DedupStore::new),
            dispatch_ctx: Arc::new(RwLock::new(DispatchContext::default())),
        }
    }

    /// Run the outbound processor using a JetStream pull consumer
    pub async fn run(self) -> Result<()> {
        use futures::StreamExt;
        use telegram_nats::nats::create_outbound_consumer;
        use telegram_nats::read_cmd_metadata;

        info!("Starting outbound processor (JetStream pull consumer)");
        let prefix = self.subscriber.prefix().to_string();

        let consumer = create_outbound_consumer(&self.js, &prefix).await?;
        let mut messages = consumer.messages().await?;

        while let Some(msg) = messages.next().await {
            let msg = msg?;
            let subject = msg.subject.as_str().to_string();
            let payload = msg.payload.clone();

            // ── Extract tracing metadata from headers (Gap B) ─────────────────
            let cmd_meta = msg.headers.as_ref().and_then(read_cmd_metadata);
            let session_id = cmd_meta.as_ref().and_then(|m| m.session_id.clone());

            // Generic chat_id extraction (most commands have it at top-level)
            #[derive(serde::Deserialize)]
            struct WithChatId {
                #[serde(default)]
                chat_id: Option<i64>,
            }
            let chat_id = serde_json::from_slice::<WithChatId>(&payload)
                .ok()
                .and_then(|w| w.chat_id);

            // Populate dispatch context for use by handle_telegram_error
            *self.dispatch_ctx.write().await = DispatchContext {
                session_id: session_id.clone(),
                chat_id,
            };

            // ── Idempotency guard (Gap A) ──────────────────────────────────────
            // Use command_id from headers when available, fall back to stream_sequence.
            let dedup_key = cmd_meta
                .as_ref()
                .map(|m| format!("cmd:{}", m.command_id))
                .or_else(|| {
                    msg.info()
                        .map(|i| format!("seq:{}", i.stream_sequence))
                        .ok()
                })
                .unwrap_or_default();

            if !dedup_key.is_empty() {
                if let Some(ref dedup) = self.dedup {
                    if dedup.is_seen(&dedup_key).await {
                        debug!("Skipping duplicate outbound command (key={})", dedup_key);
                        if let Err(e) = msg.ack().await {
                            warn!("Failed to ack duplicate command on '{}': {}", subject, e);
                        }
                        continue;
                    }
                    // Optimistic mark before Telegram API call — prevents racing redelivery
                    if let Err(e) = dedup.mark_seen(&dedup_key).await {
                        warn!("Failed to mark command as seen (key={}): {}", dedup_key, e);
                    }
                }
            }
            // ── End idempotency guard ──────────────────────────────────────────

            let attempt = msg.info().map(|i| i.delivered as u32).unwrap_or(1);

            match self.dispatch(&subject, &payload, &prefix, attempt).await {
                Ok(_) => {
                    if let Err(e) = msg.ack().await {
                        warn!("Failed to ack message on '{}': {}", subject, e);
                    }
                }
                Err(e) => {
                    if let Some(nak) = e.downcast_ref::<NakError>() {
                        // Telegram error: NAK with optional delay (respects rate-limit retry_after)
                        let delay = nak.delay;
                        if let Err(e) = msg
                            .ack_with(async_nats::jetstream::AckKind::Nak(delay))
                            .await
                        {
                            warn!("Failed to nak message on '{}': {}", subject, e);
                        }
                    } else {
                        // Serde / parsing error: permanent, ack to avoid infinite redelivery
                        warn!("Permanent dispatch error on '{}': {}", subject, e);
                        if let Err(e) = msg.ack().await {
                            warn!("Failed to ack permanent error message: {}", e);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Dispatch a message to the appropriate handler based on subject
    async fn dispatch(
        &self,
        subject: &str,
        payload: &[u8],
        prefix: &str,
        _attempt: u32,
    ) -> Result<()> {
        use telegram_nats::subjects::agent;

        if subject == agent::message_send(prefix) {
            self.handle_send_message(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::message_edit(prefix) {
            self.handle_edit_message(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::message_delete(prefix) {
            self.handle_delete_message(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::message_forward(prefix) {
            self.handle_forward_message(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::message_copy(prefix) {
            self.handle_copy_message(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::message_send_photo(prefix) {
            self.handle_send_photo(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::message_send_video(prefix) {
            self.handle_send_video(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::message_send_audio(prefix) {
            self.handle_send_audio(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::message_send_document(prefix) {
            self.handle_send_document(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::message_send_voice(prefix) {
            self.handle_send_voice(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::message_send_media_group(prefix) {
            self.handle_send_media_group(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::message_send_sticker(prefix) {
            self.handle_send_sticker(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::message_send_animation(prefix) {
            self.handle_send_animation(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::message_send_video_note(prefix) {
            self.handle_send_video_note(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::message_send_location(prefix) {
            self.handle_send_location(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::message_send_venue(prefix) {
            self.handle_send_venue(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::message_send_contact(prefix) {
            self.handle_send_contact(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::message_stream(prefix) {
            self.handle_stream_message(serde_json::from_slice(payload)?)
                .await
        } else if subject == agent::poll_send(prefix) {
            self.handle_send_poll(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::poll_stop(prefix) {
            self.handle_stop_poll(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::callback_answer(prefix) {
            self.handle_answer_callback(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::chat_action(prefix) {
            self.handle_chat_action(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::inline_answer(prefix) {
            self.handle_answer_inline_query(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::forum_create(prefix) {
            self.handle_forum_create(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::forum_edit(prefix) {
            self.handle_forum_edit(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::forum_close(prefix) {
            self.handle_forum_close(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::forum_reopen(prefix) {
            self.handle_forum_reopen(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::forum_delete(prefix) {
            self.handle_forum_delete(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::forum_unpin(prefix) {
            self.handle_forum_unpin(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::forum_edit_general(prefix) {
            self.handle_forum_edit_general(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::forum_close_general(prefix) {
            self.handle_forum_close_general(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::forum_reopen_general(prefix) {
            self.handle_forum_reopen_general(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::forum_hide_general(prefix) {
            self.handle_forum_hide_general(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::forum_unhide_general(prefix) {
            self.handle_forum_unhide_general(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::forum_unpin_general(prefix) {
            self.handle_forum_unpin_general(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::file_get(prefix) {
            self.handle_file_get(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::file_download(prefix) {
            self.handle_file_download(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::admin_promote(prefix) {
            self.handle_admin_promote(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::admin_restrict(prefix) {
            self.handle_admin_restrict(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::admin_ban(prefix) {
            self.handle_admin_ban(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::admin_unban(prefix) {
            self.handle_admin_unban(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::admin_set_permissions(prefix) {
            self.handle_admin_set_permissions(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::admin_set_title(prefix) {
            self.handle_admin_set_title(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::admin_pin(prefix) {
            self.handle_admin_pin(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::admin_unpin(prefix) {
            self.handle_admin_unpin(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::admin_unpin_all(prefix) {
            self.handle_admin_unpin_all(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::admin_set_chat_title(prefix) {
            self.handle_admin_set_chat_title(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::admin_set_chat_description(prefix) {
            self.handle_admin_set_chat_description(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::payment_send_invoice(prefix) {
            self.handle_payment_send_invoice(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::payment_answer_pre_checkout(prefix) {
            self.handle_payment_answer_pre_checkout(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::payment_answer_shipping(prefix) {
            self.handle_payment_answer_shipping(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::bot_commands_set(prefix) {
            self.handle_bot_commands_set(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::bot_commands_delete(prefix) {
            self.handle_bot_commands_delete(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::bot_commands_get(prefix) {
            self.handle_bot_commands_get(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::sticker_get_set(prefix) {
            self.handle_sticker_get_set(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::sticker_upload_file(prefix) {
            self.handle_sticker_upload_file(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::sticker_create_set(prefix) {
            self.handle_sticker_create_set(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::sticker_add_to_set(prefix) {
            self.handle_sticker_add_to_set(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::sticker_set_position(prefix) {
            self.handle_sticker_set_position(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::sticker_delete_from_set(prefix) {
            self.handle_sticker_delete_from_set(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::sticker_set_title(prefix) {
            self.handle_sticker_set_title(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::sticker_set_thumbnail(prefix) {
            self.handle_sticker_set_thumbnail(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::sticker_delete_set(prefix) {
            self.handle_sticker_delete_set(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::sticker_set_emoji_list(prefix) {
            self.handle_sticker_set_emoji_list(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::sticker_set_keywords(prefix) {
            self.handle_sticker_set_keywords(serde_json::from_slice(payload)?, prefix)
                .await
        } else if subject == agent::sticker_set_mask_position(prefix) {
            self.handle_sticker_set_mask_position(serde_json::from_slice(payload)?, prefix)
                .await
        } else {
            warn!("Unknown command subject: {}", subject);
            Ok(())
        }
    }

    // ── Message handlers ──────────────────────────────────────────────────────

    async fn handle_send_message(&self, cmd: SendMessageCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::message_send(prefix);
        debug!("Received send message command for chat {}", cmd.chat_id);

        let mut req = self.bot.send_message(ChatId(cmd.chat_id), cmd.text);

        if let Some(parse_mode) = cmd.parse_mode {
            req.parse_mode = Some(convert_parse_mode(parse_mode));
        }

        if let Some(reply_to) = cmd.reply_to_message_id {
            req.reply_parameters = Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
        }

        if let Some(markup) = cmd.reply_markup {
            req.reply_markup = Some(teloxide::types::ReplyMarkup::InlineKeyboard(
                convert_inline_keyboard(markup),
            ));
        }

        if let Some(thread_id) = cmd.message_thread_id {
            req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_edit_message(&self, cmd: EditMessageCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::message_edit(prefix);
        debug!(
            "Received edit message command for chat {} msg {}",
            cmd.chat_id, cmd.message_id
        );

        let mut req =
            self.bot
                .edit_message_text(ChatId(cmd.chat_id), MessageId(cmd.message_id), cmd.text);

        if let Some(parse_mode) = cmd.parse_mode {
            req.parse_mode = Some(convert_parse_mode(parse_mode));
        }

        if let Some(markup) = cmd.reply_markup {
            req.reply_markup = Some(convert_inline_keyboard(markup));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_delete_message(&self, cmd: DeleteMessageCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::message_delete(prefix);
        debug!(
            "Received delete message command for chat {} msg {}",
            cmd.chat_id, cmd.message_id
        );

        if let Err(e) = self
            .bot
            .delete_message(ChatId(cmd.chat_id), MessageId(cmd.message_id))
            .await
        {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_forward_message(&self, cmd: ForwardMessageCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::message_forward(prefix);
        debug!(
            "Received forward message command: from_chat={} msg={} to_chat={}",
            cmd.from_chat_id, cmd.message_id, cmd.chat_id
        );

        let mut req = self.bot.forward_message(
            ChatId(cmd.chat_id),
            ChatId(cmd.from_chat_id),
            MessageId(cmd.message_id),
        );

        if let Some(thread_id) = cmd.message_thread_id {
            req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
        }
        req.disable_notification = cmd.disable_notification;
        req.protect_content = cmd.protect_content;

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_copy_message(&self, cmd: CopyMessageCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::message_copy(prefix);
        debug!(
            "Received copy message command: from_chat={} msg={} to_chat={}",
            cmd.from_chat_id, cmd.message_id, cmd.chat_id
        );

        let mut req = self.bot.copy_message(
            ChatId(cmd.chat_id),
            ChatId(cmd.from_chat_id),
            MessageId(cmd.message_id),
        );

        if let Some(thread_id) = cmd.message_thread_id {
            req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
        }
        if let Some(caption) = cmd.caption {
            req.caption = Some(caption);
        }
        if let Some(parse_mode) = cmd.parse_mode {
            req.parse_mode = Some(match parse_mode {
                ParseMode::HTML => TgParseMode::Html,
                ParseMode::Markdown => TgParseMode::MarkdownV2,
                ParseMode::MarkdownV2 => TgParseMode::MarkdownV2,
            });
        }
        if let Some(reply_id) = cmd.reply_to_message_id {
            req.reply_parameters = Some(teloxide::types::ReplyParameters::new(MessageId(reply_id)));
        }
        req.disable_notification = cmd.disable_notification;
        req.protect_content = cmd.protect_content;

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    // ── Media handlers ────────────────────────────────────────────────────────

    async fn handle_send_photo(&self, cmd: SendPhotoCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::message_send_photo(prefix);
        debug!("Received send photo command for chat {}", cmd.chat_id);

        let photo = teloxide::types::InputFile::file_id(cmd.photo);
        let mut req = self.bot.send_photo(ChatId(cmd.chat_id), photo);

        if let Some(caption) = cmd.caption {
            req.caption = Some(caption);
        }

        if let Some(parse_mode) = cmd.parse_mode {
            req.parse_mode = Some(convert_parse_mode(parse_mode));
        }

        if let Some(reply_to) = cmd.reply_to_message_id {
            req.reply_parameters = Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
        }

        if let Some(thread_id) = cmd.message_thread_id {
            req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_send_video(&self, cmd: SendVideoCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::message_send_video(prefix);
        debug!("Received send video command for chat {}", cmd.chat_id);

        let video = teloxide::types::InputFile::file_id(cmd.video);
        let mut req = self.bot.send_video(ChatId(cmd.chat_id), video);

        if let Some(caption) = cmd.caption {
            req.caption = Some(caption);
        }

        if let Some(parse_mode) = cmd.parse_mode {
            req.parse_mode = Some(convert_parse_mode(parse_mode));
        }

        if let Some(duration) = cmd.duration {
            req.duration = Some(duration);
        }

        if let Some(width) = cmd.width {
            req.width = Some(width);
        }

        if let Some(height) = cmd.height {
            req.height = Some(height);
        }

        if let Some(supports_streaming) = cmd.supports_streaming {
            req.supports_streaming = Some(supports_streaming);
        }

        if let Some(reply_to) = cmd.reply_to_message_id {
            req.reply_parameters = Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
        }

        if let Some(thread_id) = cmd.message_thread_id {
            req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_send_audio(&self, cmd: SendAudioCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::message_send_audio(prefix);
        debug!("Received send audio command for chat {}", cmd.chat_id);

        let audio = teloxide::types::InputFile::file_id(cmd.audio);
        let mut req = self.bot.send_audio(ChatId(cmd.chat_id), audio);

        if let Some(caption) = cmd.caption {
            req.caption = Some(caption);
        }

        if let Some(parse_mode) = cmd.parse_mode {
            req.parse_mode = Some(convert_parse_mode(parse_mode));
        }

        if let Some(duration) = cmd.duration {
            req.duration = Some(duration);
        }

        if let Some(performer) = cmd.performer {
            req.performer = Some(performer);
        }

        if let Some(title) = cmd.title {
            req.title = Some(title);
        }

        if let Some(reply_to) = cmd.reply_to_message_id {
            req.reply_parameters = Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
        }

        if let Some(thread_id) = cmd.message_thread_id {
            req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_send_document(&self, cmd: SendDocumentCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::message_send_document(prefix);
        debug!("Received send document command for chat {}", cmd.chat_id);

        let document = teloxide::types::InputFile::file_id(cmd.document);
        let mut req = self.bot.send_document(ChatId(cmd.chat_id), document);

        if let Some(caption) = cmd.caption {
            req.caption = Some(caption);
        }

        if let Some(parse_mode) = cmd.parse_mode {
            req.parse_mode = Some(convert_parse_mode(parse_mode));
        }

        if let Some(reply_to) = cmd.reply_to_message_id {
            req.reply_parameters = Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
        }

        if let Some(thread_id) = cmd.message_thread_id {
            req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_send_voice(&self, cmd: SendVoiceCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::message_send_voice(prefix);
        debug!("Received send voice command for chat {}", cmd.chat_id);

        let voice = teloxide::types::InputFile::file_id(cmd.voice);
        let mut req = self.bot.send_voice(ChatId(cmd.chat_id), voice);

        if let Some(caption) = cmd.caption {
            req.caption = Some(caption);
        }

        if let Some(parse_mode) = cmd.parse_mode {
            req.parse_mode = Some(convert_parse_mode(parse_mode));
        }

        if let Some(duration) = cmd.duration {
            req.duration = Some(duration);
        }

        if let Some(reply_to) = cmd.reply_to_message_id {
            req.reply_parameters = Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
        }

        if let Some(thread_id) = cmd.message_thread_id {
            req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_send_media_group(
        &self,
        cmd: SendMediaGroupCommand,
        prefix: &str,
    ) -> Result<()> {
        use telegram_types::commands::InputMediaItem;
        use teloxide::types::{
            InputMedia, InputMediaAudio, InputMediaDocument, InputMediaPhoto, InputMediaVideo,
        };

        let subject = subjects::agent::message_send_media_group(prefix);
        debug!(
            "Received send media group command for chat {} ({} items)",
            cmd.chat_id,
            cmd.media.len()
        );

        let media: Vec<InputMedia> = cmd
            .media
            .into_iter()
            .map(|item| match item {
                InputMediaItem::Photo {
                    media,
                    caption,
                    parse_mode,
                } => {
                    let mut m = InputMediaPhoto::new(teloxide::types::InputFile::file_id(media));
                    m.caption = caption;
                    m.parse_mode = parse_mode.map(convert_parse_mode);
                    InputMedia::Photo(m)
                }
                InputMediaItem::Video {
                    media,
                    caption,
                    parse_mode,
                    duration,
                    width,
                    height,
                    supports_streaming,
                } => {
                    let mut m = InputMediaVideo::new(teloxide::types::InputFile::file_id(media));
                    m.caption = caption;
                    m.parse_mode = parse_mode.map(convert_parse_mode);
                    m.duration = duration.map(|d| d as u16);
                    m.width = width.map(|w| w as u16);
                    m.height = height.map(|h| h as u16);
                    m.supports_streaming = supports_streaming;
                    InputMedia::Video(m)
                }
                InputMediaItem::Audio {
                    media,
                    caption,
                    parse_mode,
                    duration,
                    performer,
                    title,
                } => {
                    let mut m = InputMediaAudio::new(teloxide::types::InputFile::file_id(media));
                    m.caption = caption;
                    m.parse_mode = parse_mode.map(convert_parse_mode);
                    m.duration = duration.map(|d| d as u16);
                    m.performer = performer;
                    m.title = title;
                    InputMedia::Audio(m)
                }
                InputMediaItem::Document {
                    media,
                    caption,
                    parse_mode,
                } => {
                    let mut m = InputMediaDocument::new(teloxide::types::InputFile::file_id(media));
                    m.caption = caption;
                    m.parse_mode = parse_mode.map(convert_parse_mode);
                    InputMedia::Document(m)
                }
            })
            .collect();

        let mut req = self.bot.send_media_group(ChatId(cmd.chat_id), media);

        if let Some(reply_to) = cmd.reply_to_message_id {
            req.reply_parameters = Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
        }

        if let Some(thread_id) = cmd.message_thread_id {
            req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_send_sticker(&self, cmd: SendStickerCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::message_send_sticker(prefix);
        debug!("Received send sticker command for chat {}", cmd.chat_id);

        let sticker = teloxide::types::InputFile::file_id(cmd.sticker);
        let mut req = self.bot.send_sticker(ChatId(cmd.chat_id), sticker);

        if let Some(reply_to) = cmd.reply_to_message_id {
            req.reply_parameters = Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
        }

        if let Some(markup) = cmd.reply_markup {
            req.reply_markup = Some(teloxide::types::ReplyMarkup::InlineKeyboard(
                convert_inline_keyboard(markup),
            ));
        }

        if let Some(thread_id) = cmd.message_thread_id {
            req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_send_animation(&self, cmd: SendAnimationCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::message_send_animation(prefix);
        debug!("Received send animation command for chat {}", cmd.chat_id);

        let animation = teloxide::types::InputFile::file_id(cmd.animation);
        let mut req = self.bot.send_animation(ChatId(cmd.chat_id), animation);

        if let Some(caption) = cmd.caption {
            req.caption = Some(caption);
        }

        if let Some(parse_mode) = cmd.parse_mode {
            req.parse_mode = Some(convert_parse_mode(parse_mode));
        }

        if let Some(duration) = cmd.duration {
            req.duration = Some(duration);
        }

        if let Some(width) = cmd.width {
            req.width = Some(width);
        }

        if let Some(height) = cmd.height {
            req.height = Some(height);
        }

        if let Some(reply_to) = cmd.reply_to_message_id {
            req.reply_parameters = Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
        }

        if let Some(markup) = cmd.reply_markup {
            req.reply_markup = Some(teloxide::types::ReplyMarkup::InlineKeyboard(
                convert_inline_keyboard(markup),
            ));
        }

        if let Some(thread_id) = cmd.message_thread_id {
            req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_send_video_note(&self, cmd: SendVideoNoteCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::message_send_video_note(prefix);
        debug!("Received send video note command for chat {}", cmd.chat_id);

        let video_note = teloxide::types::InputFile::file_id(cmd.video_note);
        let mut req = self.bot.send_video_note(ChatId(cmd.chat_id), video_note);

        if let Some(duration) = cmd.duration {
            req.duration = Some(duration);
        }

        if let Some(length) = cmd.length {
            req.length = Some(length);
        }

        if let Some(reply_to) = cmd.reply_to_message_id {
            req.reply_parameters = Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
        }

        if let Some(thread_id) = cmd.message_thread_id {
            req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_send_location(&self, cmd: SendLocationCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::message_send_location(prefix);
        debug!("Received send location command for chat {}", cmd.chat_id);

        let mut req = self
            .bot
            .send_location(ChatId(cmd.chat_id), cmd.latitude, cmd.longitude);

        if let Some(live_period) = cmd.live_period {
            req.live_period = Some(live_period.into());
        }

        if let Some(horizontal_accuracy) = cmd.horizontal_accuracy {
            req.horizontal_accuracy = Some(horizontal_accuracy);
        }

        if let Some(heading) = cmd.heading {
            req.heading = Some(heading);
        }

        if let Some(proximity_alert_radius) = cmd.proximity_alert_radius {
            req.proximity_alert_radius = Some(proximity_alert_radius);
        }

        if let Some(reply_to) = cmd.reply_to_message_id {
            req.reply_parameters = Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
        }

        if let Some(thread_id) = cmd.message_thread_id {
            req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_send_venue(&self, cmd: SendVenueCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::message_send_venue(prefix);
        debug!("Received send venue command for chat {}", cmd.chat_id);

        let mut req = self.bot.send_venue(
            ChatId(cmd.chat_id),
            cmd.latitude,
            cmd.longitude,
            cmd.title,
            cmd.address,
        );

        if let Some(foursquare_id) = cmd.foursquare_id {
            req.foursquare_id = Some(foursquare_id);
        }

        if let Some(foursquare_type) = cmd.foursquare_type {
            req.foursquare_type = Some(foursquare_type);
        }

        if let Some(google_place_id) = cmd.google_place_id {
            req.google_place_id = Some(google_place_id);
        }

        if let Some(google_place_type) = cmd.google_place_type {
            req.google_place_type = Some(google_place_type);
        }

        if let Some(reply_to) = cmd.reply_to_message_id {
            req.reply_parameters = Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
        }

        if let Some(thread_id) = cmd.message_thread_id {
            req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_send_contact(&self, cmd: SendContactCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::message_send_contact(prefix);
        debug!("Received send contact command for chat {}", cmd.chat_id);

        let mut req = self
            .bot
            .send_contact(ChatId(cmd.chat_id), cmd.phone_number, cmd.first_name);

        if let Some(last_name) = cmd.last_name {
            req.last_name = Some(last_name);
        }

        if let Some(vcard) = cmd.vcard {
            req.vcard = Some(vcard);
        }

        if let Some(reply_to) = cmd.reply_to_message_id {
            req.reply_parameters = Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
        }

        if let Some(thread_id) = cmd.message_thread_id {
            req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    // ── Poll handlers ─────────────────────────────────────────────────────────

    async fn handle_send_poll(&self, cmd: SendPollCommand, prefix: &str) -> Result<()> {
        use telegram_types::commands::PollKind;
        use teloxide::types::PollType as TgPollType;

        let subject = subjects::agent::poll_send(prefix);
        debug!("Received send poll command for chat {}", cmd.chat_id);

        let options: Vec<teloxide::types::InputPollOption> = cmd
            .options
            .into_iter()
            .map(teloxide::types::InputPollOption::new)
            .collect();

        let poll_type = match cmd.poll_type {
            Some(PollKind::Quiz) => TgPollType::Quiz,
            _ => TgPollType::Regular,
        };

        let mut req = self
            .bot
            .send_poll(ChatId(cmd.chat_id), cmd.question, options);

        req.is_anonymous = cmd.is_anonymous;
        req.type_ = Some(poll_type);
        req.allows_multiple_answers = cmd.allows_multiple_answers;
        req.correct_option_id = cmd.correct_option_id;

        if let Some(explanation) = cmd.explanation {
            req.explanation = Some(explanation);
        }

        if let Some(pm) = cmd.explanation_parse_mode {
            req.explanation_parse_mode = Some(convert_parse_mode(pm));
        }

        if let Some(period) = cmd.open_period {
            req.open_period = Some(period);
        }

        req.is_closed = cmd.is_closed;

        if let Some(reply_to) = cmd.reply_to_message_id {
            req.reply_parameters = Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
        }

        if let Some(thread_id) = cmd.message_thread_id {
            req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_stop_poll(&self, cmd: StopPollCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::poll_stop(prefix);
        debug!("Received stop poll command for chat {}", cmd.chat_id);

        let mut req = self
            .bot
            .stop_poll(ChatId(cmd.chat_id), MessageId(cmd.message_id));

        if let Some(markup) = cmd.reply_markup {
            req.reply_markup = Some(convert_inline_keyboard(markup));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    // ── Callback / action / inline handlers ───────────────────────────────────

    async fn handle_answer_callback(&self, cmd: AnswerCallbackCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::callback_answer(prefix);
        debug!("Received answer callback command");

        let mut req = self.bot.answer_callback_query(&cmd.callback_query_id);

        if let Some(text) = cmd.text {
            req.text = Some(text);
        }

        if let Some(show_alert) = cmd.show_alert {
            req.show_alert = Some(show_alert);
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_chat_action(&self, cmd: SendChatActionCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::chat_action(prefix);
        debug!("Received chat action command for chat {}", cmd.chat_id);

        let action = convert_chat_action(cmd.action);
        let mut req = self.bot.send_chat_action(ChatId(cmd.chat_id), action);

        if let Some(thread_id) = cmd.message_thread_id {
            req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_answer_inline_query(
        &self,
        cmd: AnswerInlineQueryCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::inline_answer(prefix);
        debug!(
            "Received answer inline query command for query {}",
            cmd.inline_query_id
        );

        let results: Vec<teloxide::types::InlineQueryResult> = cmd
            .results
            .into_iter()
            .filter_map(convert_inline_query_result)
            .collect();

        let mut req = self.bot.answer_inline_query(&cmd.inline_query_id, results);

        if let Some(cache_time) = cmd.cache_time {
            req.cache_time = Some(cache_time as u32);
        }

        if let Some(is_personal) = cmd.is_personal {
            req.is_personal = Some(is_personal);
        }

        if let Some(next_offset) = cmd.next_offset {
            req.next_offset = Some(next_offset);
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    // ── Forum topic handlers ──────────────────────────────────────────────────

    async fn handle_forum_create(&self, cmd: CreateForumTopicCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::forum_create(prefix);
        debug!("Creating forum topic in chat {}", cmd.chat_id);

        let icon_color = cmd
            .icon_color
            .map(|c| teloxide::types::Rgb::from_u32(c as u32))
            .unwrap_or_else(|| teloxide::types::Rgb::from_u32(0x6FB9F0));
        let icon_custom_emoji_id = cmd.icon_custom_emoji_id.unwrap_or_default();
        let req = self.bot.create_forum_topic(
            ChatId(cmd.chat_id),
            cmd.name,
            icon_color,
            icon_custom_emoji_id,
        );
        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_forum_edit(&self, cmd: EditForumTopicCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::forum_edit(prefix);
        debug!(
            "Editing forum topic {} in chat {}",
            cmd.message_thread_id, cmd.chat_id
        );

        let mut req = self.bot.edit_forum_topic(
            ChatId(cmd.chat_id),
            teloxide::types::ThreadId(MessageId(cmd.message_thread_id)),
        );
        if let Some(name) = cmd.name {
            req.name = Some(name);
        }
        if let Some(emoji_id) = cmd.icon_custom_emoji_id {
            req.icon_custom_emoji_id = Some(emoji_id);
        }
        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_forum_close(&self, cmd: CloseForumTopicCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::forum_close(prefix);
        debug!(
            "Closing forum topic {} in chat {}",
            cmd.message_thread_id, cmd.chat_id
        );

        if let Err(e) = self
            .bot
            .close_forum_topic(
                ChatId(cmd.chat_id),
                teloxide::types::ThreadId(MessageId(cmd.message_thread_id)),
            )
            .await
        {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_forum_reopen(&self, cmd: ReopenForumTopicCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::forum_reopen(prefix);
        debug!(
            "Reopening forum topic {} in chat {}",
            cmd.message_thread_id, cmd.chat_id
        );

        if let Err(e) = self
            .bot
            .reopen_forum_topic(
                ChatId(cmd.chat_id),
                teloxide::types::ThreadId(MessageId(cmd.message_thread_id)),
            )
            .await
        {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_forum_delete(&self, cmd: DeleteForumTopicCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::forum_delete(prefix);
        debug!(
            "Deleting forum topic {} in chat {}",
            cmd.message_thread_id, cmd.chat_id
        );

        if let Err(e) = self
            .bot
            .delete_forum_topic(
                ChatId(cmd.chat_id),
                teloxide::types::ThreadId(MessageId(cmd.message_thread_id)),
            )
            .await
        {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_forum_unpin(
        &self,
        cmd: UnpinAllForumTopicMessagesCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::forum_unpin(prefix);
        debug!(
            "Unpinning all messages in topic {} in chat {}",
            cmd.message_thread_id, cmd.chat_id
        );

        if let Err(e) = self
            .bot
            .unpin_all_forum_topic_messages(
                ChatId(cmd.chat_id),
                teloxide::types::ThreadId(MessageId(cmd.message_thread_id)),
            )
            .await
        {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_forum_edit_general(
        &self,
        cmd: EditGeneralForumTopicCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::forum_edit_general(prefix);
        debug!("Editing general forum topic in chat {}", cmd.chat_id);

        if let Err(e) = self
            .bot
            .edit_general_forum_topic(ChatId(cmd.chat_id), cmd.name)
            .await
        {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_forum_close_general(
        &self,
        cmd: CloseGeneralForumTopicCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::forum_close_general(prefix);
        debug!("Closing general forum topic in chat {}", cmd.chat_id);

        if let Err(e) = self
            .bot
            .close_general_forum_topic(ChatId(cmd.chat_id))
            .await
        {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_forum_reopen_general(
        &self,
        cmd: ReopenGeneralForumTopicCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::forum_reopen_general(prefix);
        debug!("Reopening general forum topic in chat {}", cmd.chat_id);

        if let Err(e) = self
            .bot
            .reopen_general_forum_topic(ChatId(cmd.chat_id))
            .await
        {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_forum_hide_general(
        &self,
        cmd: HideGeneralForumTopicCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::forum_hide_general(prefix);
        debug!("Hiding general forum topic in chat {}", cmd.chat_id);

        if let Err(e) = self.bot.hide_general_forum_topic(ChatId(cmd.chat_id)).await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_forum_unhide_general(
        &self,
        cmd: UnhideGeneralForumTopicCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::forum_unhide_general(prefix);
        debug!("Unhiding general forum topic in chat {}", cmd.chat_id);

        if let Err(e) = self
            .bot
            .unhide_general_forum_topic(ChatId(cmd.chat_id))
            .await
        {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_forum_unpin_general(
        &self,
        cmd: UnpinAllGeneralForumTopicMessagesCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::forum_unpin_general(prefix);
        debug!(
            "Unpinning all general forum topic messages in chat {}",
            cmd.chat_id
        );

        if let Err(e) = self
            .bot
            .unpin_all_general_forum_topic_messages(ChatId(cmd.chat_id))
            .await
        {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    // ── File handlers ─────────────────────────────────────────────────────────

    async fn handle_file_get(&self, cmd: GetFileCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::file_get(prefix);
        debug!("Getting file info for file_id: {}", cmd.file_id);

        match self.bot.get_file(&cmd.file_id).await {
            Ok(file) => {
                let bot_token = self.bot.token();
                let download_url = format!(
                    "https://api.telegram.org/file/bot{}/{}",
                    bot_token, file.path
                );

                let response = FileInfoResponse {
                    file_id: cmd.file_id.clone(),
                    file_unique_id: file.unique_id.clone(),
                    file_size: Some(file.size as u64),
                    file_path: file.path.clone(),
                    download_url,
                    request_id: cmd.request_id,
                };

                let response_subject = subjects::bot::file_info(prefix);
                if let Err(e) = self
                    .subscriber
                    .client()
                    .publish(
                        response_subject.clone(),
                        serde_json::to_vec(&response).unwrap().into(),
                    )
                    .await
                {
                    error!("Failed to publish file info response: {}", e);
                } else {
                    debug!("Published file info response to {}", response_subject);
                }
            }
            Err(e) => {
                self.handle_telegram_error(&subject, e, prefix).await?;
            }
        }

        Ok(())
    }

    async fn handle_file_download(&self, cmd: DownloadFileCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::file_download(prefix);
        debug!(
            "Downloading file {} to {}",
            cmd.file_id, cmd.destination_path
        );

        match self.bot.get_file(&cmd.file_id).await {
            Ok(file) => {
                let bot_token = self.bot.token();
                let download_url = format!(
                    "https://api.telegram.org/file/bot{}/{}",
                    bot_token, file.path
                );

                match download_file_from_url(&download_url, &cmd.destination_path).await {
                    Ok(file_size) => {
                        let response = FileDownloadResponse {
                            file_id: cmd.file_id.clone(),
                            local_path: cmd.destination_path.clone(),
                            file_size,
                            success: true,
                            error: None,
                            request_id: cmd.request_id,
                        };

                        let response_subject = subjects::bot::file_downloaded(prefix);
                        if let Err(e) = self
                            .subscriber
                            .client()
                            .publish(
                                response_subject.clone(),
                                serde_json::to_vec(&response).unwrap().into(),
                            )
                            .await
                        {
                            error!("Failed to publish file download response: {}", e);
                        } else {
                            debug!("Published file download response to {}", response_subject);
                        }
                    }
                    Err(e) => {
                        error!("Failed to download file {}: {}", cmd.file_id, e);
                        let response = FileDownloadResponse {
                            file_id: cmd.file_id.clone(),
                            local_path: cmd.destination_path.clone(),
                            file_size: 0,
                            success: false,
                            error: Some(e.to_string()),
                            request_id: cmd.request_id,
                        };

                        let response_subject = subjects::bot::file_downloaded(prefix);
                        if let Err(e) = self
                            .subscriber
                            .client()
                            .publish(
                                response_subject.clone(),
                                serde_json::to_vec(&response).unwrap().into(),
                            )
                            .await
                        {
                            error!("Failed to publish file download error response: {}", e);
                        }
                    }
                }
            }
            Err(e) => {
                let e_str = e.to_string();
                self.handle_telegram_error(&subject, e, prefix).await?;
                let response = FileDownloadResponse {
                    file_id: cmd.file_id.clone(),
                    local_path: cmd.destination_path.clone(),
                    file_size: 0,
                    success: false,
                    error: Some(format!("Failed to get file info: {}", e_str)),
                    request_id: cmd.request_id,
                };

                let response_subject = subjects::bot::file_downloaded(prefix);
                if let Err(e) = self
                    .subscriber
                    .client()
                    .publish(
                        response_subject.clone(),
                        serde_json::to_vec(&response).unwrap().into(),
                    )
                    .await
                {
                    error!("Failed to publish file download error response: {}", e);
                }
            }
        }

        Ok(())
    }

    // ── Admin handlers ────────────────────────────────────────────────────────

    async fn handle_admin_promote(
        &self,
        cmd: PromoteChatMemberCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::admin_promote(prefix);
        debug!("Promoting user {} in chat {}", cmd.user_id, cmd.chat_id);

        let mut req = self.bot.promote_chat_member(
            ChatId(cmd.chat_id),
            teloxide::types::UserId(cmd.user_id as u64),
        );

        if let Some(rights) = cmd.rights {
            req.is_anonymous = rights.is_anonymous;
            req.can_manage_chat = rights.can_manage_chat;
            req.can_delete_messages = rights.can_delete_messages;
            req.can_manage_video_chats = rights.can_manage_video_chats;
            req.can_restrict_members = rights.can_restrict_members;
            req.can_promote_members = rights.can_promote_members;
            req.can_change_info = rights.can_change_info;
            req.can_invite_users = rights.can_invite_users;
            req.can_pin_messages = rights.can_pin_messages;
            req.can_manage_topics = rights.can_manage_topics;
            req.can_post_messages = rights.can_post_messages;
            req.can_edit_messages = rights.can_edit_messages;
            req.can_post_stories = rights.can_post_stories;
            req.can_edit_stories = rights.can_edit_stories;
            req.can_delete_stories = rights.can_delete_stories;
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_admin_restrict(
        &self,
        cmd: RestrictChatMemberCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::admin_restrict(prefix);
        debug!("Restricting user {} in chat {}", cmd.user_id, cmd.chat_id);

        let perms = convert_chat_permissions(&cmd.permissions);
        let mut req = self.bot.restrict_chat_member(
            ChatId(cmd.chat_id),
            teloxide::types::UserId(cmd.user_id as u64),
            perms,
        );

        if let Some(until) = cmd.until_date {
            use chrono::{DateTime, Utc};
            if let Some(dt) = DateTime::<Utc>::from_timestamp(until, 0) {
                req.until_date = Some(dt);
            }
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_admin_ban(&self, cmd: BanChatMemberCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::admin_ban(prefix);
        debug!("Banning user {} in chat {}", cmd.user_id, cmd.chat_id);

        let mut req = self.bot.ban_chat_member(
            ChatId(cmd.chat_id),
            teloxide::types::UserId(cmd.user_id as u64),
        );

        if let Some(until) = cmd.until_date {
            use chrono::{DateTime, Utc};
            if let Some(dt) = DateTime::<Utc>::from_timestamp(until, 0) {
                req.until_date = Some(dt);
            }
        }

        if let Some(revoke) = cmd.revoke_messages {
            req.revoke_messages = Some(revoke);
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_admin_unban(&self, cmd: UnbanChatMemberCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::admin_unban(prefix);
        debug!("Unbanning user {} in chat {}", cmd.user_id, cmd.chat_id);

        let mut req = self.bot.unban_chat_member(
            ChatId(cmd.chat_id),
            teloxide::types::UserId(cmd.user_id as u64),
        );

        if let Some(only_if_banned) = cmd.only_if_banned {
            req.only_if_banned = Some(only_if_banned);
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_admin_set_permissions(
        &self,
        cmd: SetChatPermissionsCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::admin_set_permissions(prefix);
        debug!("Setting permissions in chat {}", cmd.chat_id);

        let perms = convert_chat_permissions(&cmd.permissions);

        if let Err(e) = self
            .bot
            .set_chat_permissions(ChatId(cmd.chat_id), perms)
            .await
        {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_admin_set_title(
        &self,
        cmd: SetChatAdministratorCustomTitleCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::admin_set_title(prefix);
        debug!(
            "Setting custom title for user {} in chat {}",
            cmd.user_id, cmd.chat_id
        );

        if let Err(e) = self
            .bot
            .set_chat_administrator_custom_title(
                ChatId(cmd.chat_id),
                teloxide::types::UserId(cmd.user_id as u64),
                cmd.custom_title,
            )
            .await
        {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_admin_pin(&self, cmd: PinChatMessageCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::admin_pin(prefix);
        debug!("Pinning message {} in chat {}", cmd.message_id, cmd.chat_id);

        let mut req = self
            .bot
            .pin_chat_message(ChatId(cmd.chat_id), MessageId(cmd.message_id));

        if let Some(disable_notification) = cmd.disable_notification {
            req.disable_notification = Some(disable_notification);
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_admin_unpin(&self, cmd: UnpinChatMessageCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::admin_unpin(prefix);
        debug!("Unpinning message in chat {}", cmd.chat_id);

        let mut req = self.bot.unpin_chat_message(ChatId(cmd.chat_id));

        if let Some(message_id) = cmd.message_id {
            req.message_id = Some(MessageId(message_id));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_admin_unpin_all(
        &self,
        cmd: UnpinAllChatMessagesCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::admin_unpin_all(prefix);
        debug!("Unpinning all messages in chat {}", cmd.chat_id);

        if let Err(e) = self.bot.unpin_all_chat_messages(ChatId(cmd.chat_id)).await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_admin_set_chat_title(
        &self,
        cmd: SetChatTitleCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::admin_set_chat_title(prefix);
        debug!("Setting chat title in chat {}", cmd.chat_id);

        if let Err(e) = self
            .bot
            .set_chat_title(ChatId(cmd.chat_id), cmd.title)
            .await
        {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_admin_set_chat_description(
        &self,
        cmd: SetChatDescriptionCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::admin_set_chat_description(prefix);
        debug!("Setting chat description in chat {}", cmd.chat_id);

        let mut req = self.bot.set_chat_description(ChatId(cmd.chat_id));
        req.description = Some(cmd.description);

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    // ── Payment handlers ──────────────────────────────────────────────────────

    async fn handle_payment_send_invoice(
        &self,
        cmd: SendInvoiceCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::payment_send_invoice(prefix);
        debug!("Sending invoice to chat {}", cmd.chat_id);

        let prices: Vec<teloxide::types::LabeledPrice> = cmd
            .prices
            .iter()
            .map(|p| teloxide::types::LabeledPrice {
                label: p.label.clone(),
                amount: p.amount as u32,
            })
            .collect();

        let mut req = self.bot.send_invoice(
            ChatId(cmd.chat_id),
            cmd.title,
            cmd.description,
            cmd.payload,
            cmd.currency,
            prices,
        );

        if !cmd.provider_token.is_empty() {
            req.provider_token = Some(cmd.provider_token);
        }
        req.max_tip_amount = cmd.max_tip_amount.map(|a| a as u32);
        req.suggested_tip_amounts = cmd
            .suggested_tip_amounts
            .map(|amounts| amounts.iter().map(|&a| a as u32).collect());
        req.start_parameter = cmd.start_parameter;
        req.provider_data = cmd.provider_data;
        req.photo_url = cmd.photo_url.and_then(|u| u.parse().ok());
        req.photo_size = cmd.photo_size.map(|s| s as u32);
        req.photo_width = cmd.photo_width.map(|w| w as u32);
        req.photo_height = cmd.photo_height.map(|h| h as u32);
        req.need_name = cmd.need_name;
        req.need_phone_number = cmd.need_phone_number;
        req.need_email = cmd.need_email;
        req.need_shipping_address = cmd.need_shipping_address;
        req.send_phone_number_to_provider = cmd.send_phone_number_to_provider;
        req.send_email_to_provider = cmd.send_email_to_provider;
        req.is_flexible = cmd.is_flexible;
        req.disable_notification = cmd.disable_notification;
        req.protect_content = cmd.protect_content;

        if let Some(reply_to) = cmd.reply_to_message_id {
            req.reply_parameters = Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
        }

        if let Some(markup) = cmd.reply_markup {
            req.reply_markup = Some(convert_inline_keyboard(markup));
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_payment_answer_pre_checkout(
        &self,
        cmd: AnswerPreCheckoutQueryCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::payment_answer_pre_checkout(prefix);
        debug!(
            "Answering pre-checkout query {}: ok={}",
            cmd.pre_checkout_query_id, cmd.ok
        );

        let req = if cmd.ok {
            self.bot
                .answer_pre_checkout_query(&cmd.pre_checkout_query_id, true)
        } else {
            let mut req = self
                .bot
                .answer_pre_checkout_query(&cmd.pre_checkout_query_id, false);
            req.error_message = cmd.error_message;
            req
        };

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_payment_answer_shipping(
        &self,
        cmd: AnswerShippingQueryCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::payment_answer_shipping(prefix);
        debug!(
            "Answering shipping query {}: ok={}",
            cmd.shipping_query_id, cmd.ok
        );

        let mut req = self
            .bot
            .answer_shipping_query(&cmd.shipping_query_id, cmd.ok);

        if cmd.ok {
            let options: Vec<teloxide::types::ShippingOption> = cmd
                .shipping_options
                .unwrap_or_default()
                .into_iter()
                .map(|opt| teloxide::types::ShippingOption {
                    id: opt.id,
                    title: opt.title,
                    prices: opt
                        .prices
                        .iter()
                        .map(|p| teloxide::types::LabeledPrice {
                            label: p.label.clone(),
                            amount: p.amount as u32,
                        })
                        .collect(),
                })
                .collect();
            req.shipping_options = Some(options);
        } else {
            req.error_message = cmd.error_message;
        }

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    // ── Bot commands handlers ─────────────────────────────────────────────────

    async fn handle_bot_commands_set(&self, cmd: SetMyCommandsCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::bot_commands_set(prefix);
        debug!("Setting bot commands ({} commands)", cmd.commands.len());

        let commands: Vec<teloxide::types::BotCommand> = cmd
            .commands
            .iter()
            .map(|c| teloxide::types::BotCommand::new(c.command.clone(), c.description.clone()))
            .collect();

        let mut req = self.bot.set_my_commands(commands);

        if let Some(scope) = cmd.scope {
            req.scope = Some(convert_bot_command_scope(scope));
        }

        req.language_code = cmd.language_code;

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_bot_commands_delete(
        &self,
        cmd: DeleteMyCommandsCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::bot_commands_delete(prefix);
        debug!("Deleting bot commands");

        let mut req = self.bot.delete_my_commands();

        if let Some(scope) = cmd.scope {
            req.scope = Some(convert_bot_command_scope(scope));
        }

        req.language_code = cmd.language_code;

        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_bot_commands_get(&self, cmd: GetMyCommandsCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::bot_commands_get(prefix);
        debug!("Getting bot commands");

        let mut req = self.bot.get_my_commands();

        if let Some(scope) = cmd.scope {
            req.scope = Some(convert_bot_command_scope(scope));
        }

        req.language_code = cmd.language_code;

        match req.await {
            Ok(commands) => {
                let response = BotCommandsResponse {
                    commands: commands
                        .iter()
                        .map(|c| telegram_types::chat::BotCommand {
                            command: c.command.clone(),
                            description: c.description.clone(),
                        })
                        .collect(),
                    request_id: cmd.request_id,
                };

                let response_subject = subjects::bot::bot_commands_response(prefix);
                if let Err(e) = self
                    .subscriber
                    .client()
                    .publish(
                        response_subject.clone(),
                        serde_json::to_vec(&response).unwrap().into(),
                    )
                    .await
                {
                    error!("Failed to publish bot commands response: {}", e);
                } else {
                    debug!("Published bot commands response to {}", response_subject);
                }
            }
            Err(e) => {
                self.handle_telegram_error(&subject, e, prefix).await?;
            }
        }

        Ok(())
    }

    // ── Sticker management handlers ───────────────────────────────────────────

    async fn handle_sticker_get_set(&self, cmd: GetStickerSetCommand, prefix: &str) -> Result<()> {
        let subject = subjects::agent::sticker_get_set(prefix);
        debug!("Getting sticker set: {}", cmd.name);

        match self.bot.get_sticker_set(&cmd.name).await {
            Ok(set) => {
                let response = StickerSetResponse {
                    sticker_set: telegram_types::chat::StickerSet {
                        name: set.name.clone(),
                        title: set.title.clone(),
                        kind: format!("{:?}", set.kind).to_lowercase(),
                        sticker_file_ids: set.stickers.iter().map(|s| s.file.id.clone()).collect(),
                        thumbnail_file_id: set.thumbnail.as_ref().map(|t| t.file.id.clone()),
                    },
                    request_id: cmd.request_id,
                };
                let subj = subjects::bot::sticker_set_info(prefix);
                if let Ok(payload) = serde_json::to_vec(&response) {
                    let _ = self.subscriber.client().publish(subj, payload.into()).await;
                }
            }
            Err(e) => self.handle_telegram_error(&subject, e, prefix).await?,
        }

        Ok(())
    }

    async fn handle_sticker_upload_file(
        &self,
        cmd: UploadStickerFileCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::sticker_upload_file(prefix);
        debug!("Uploading sticker file for user {}", cmd.user_id);

        let sticker_file = teloxide::types::InputFile::file_id(&cmd.sticker);
        let format = convert_sticker_format(&cmd.format);
        match self
            .bot
            .upload_sticker_file(
                teloxide::types::UserId(cmd.user_id as u64),
                sticker_file,
                format,
            )
            .await
        {
            Ok(file) => {
                let response = UploadStickerFileResponse {
                    file_id: file.id.clone(),
                    request_id: cmd.request_id,
                };
                let subj = subjects::bot::sticker_uploaded(prefix);
                if let Ok(payload) = serde_json::to_vec(&response) {
                    let _ = self.subscriber.client().publish(subj, payload.into()).await;
                }
            }
            Err(e) => self.handle_telegram_error(&subject, e, prefix).await?,
        }

        Ok(())
    }

    async fn handle_sticker_create_set(
        &self,
        cmd: CreateNewStickerSetCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::sticker_create_set(prefix);
        debug!(
            "Creating sticker set '{}' for user {}",
            cmd.name, cmd.user_id
        );

        let stickers: Vec<teloxide::types::InputSticker> =
            cmd.stickers.iter().map(convert_input_sticker).collect();
        let mut req = self.bot.create_new_sticker_set(
            teloxide::types::UserId(cmd.user_id as u64),
            &cmd.name,
            &cmd.title,
            stickers,
        );
        if let Some(ref kind) = cmd.sticker_type {
            req.sticker_type = Some(convert_sticker_type(kind));
        }
        req.needs_repainting = cmd.needs_repainting;
        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_sticker_add_to_set(
        &self,
        cmd: AddStickerToSetCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::sticker_add_to_set(prefix);
        debug!(
            "Adding sticker to set '{}' for user {}",
            cmd.name, cmd.user_id
        );

        let sticker = convert_input_sticker(&cmd.sticker);
        if let Err(e) = self
            .bot
            .add_sticker_to_set(
                teloxide::types::UserId(cmd.user_id as u64),
                &cmd.name,
                sticker,
            )
            .await
        {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_sticker_set_position(
        &self,
        cmd: SetStickerPositionInSetCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::sticker_set_position(prefix);
        debug!("Setting sticker position to {}", cmd.position);

        if let Err(e) = self
            .bot
            .set_sticker_position_in_set(&cmd.sticker, cmd.position)
            .await
        {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_sticker_delete_from_set(
        &self,
        cmd: DeleteStickerFromSetCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::sticker_delete_from_set(prefix);
        debug!("Deleting sticker from set: {}", cmd.sticker);

        if let Err(e) = self.bot.delete_sticker_from_set(&cmd.sticker).await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_sticker_set_title(
        &self,
        cmd: SetStickerSetTitleCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::sticker_set_title(prefix);
        debug!(
            "Setting title of sticker set '{}' to '{}'",
            cmd.name, cmd.title
        );

        if let Err(e) = self.bot.set_sticker_set_title(&cmd.name, &cmd.title).await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_sticker_set_thumbnail(
        &self,
        cmd: SetStickerSetThumbnailCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::sticker_set_thumbnail(prefix);
        debug!("Setting thumbnail of sticker set '{}'", cmd.name);

        let format = convert_sticker_format(&cmd.format);
        let mut req = self.bot.set_sticker_set_thumbnail(
            &cmd.name,
            teloxide::types::UserId(cmd.user_id as u64),
            format,
        );
        if let Some(ref file_id) = cmd.thumbnail {
            req.thumbnail = Some(teloxide::types::InputFile::file_id(file_id));
        }
        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_sticker_delete_set(
        &self,
        cmd: DeleteStickerSetCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::sticker_delete_set(prefix);
        debug!("Deleting sticker set '{}'", cmd.name);

        if let Err(e) = self.bot.delete_sticker_set(&cmd.name).await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_sticker_set_emoji_list(
        &self,
        cmd: SetStickerEmojiListCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::sticker_set_emoji_list(prefix);
        debug!("Setting emoji list for sticker {}", cmd.sticker);

        if let Err(e) = self
            .bot
            .set_sticker_emoji_list(&cmd.sticker, cmd.emoji_list)
            .await
        {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_sticker_set_keywords(
        &self,
        cmd: SetStickerKeywordsCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::sticker_set_keywords(prefix);
        debug!("Setting keywords for sticker {}", cmd.sticker);

        let mut req = self.bot.set_sticker_keywords(&cmd.sticker);
        req.keywords = Some(cmd.keywords);
        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    async fn handle_sticker_set_mask_position(
        &self,
        cmd: SetStickerMaskPositionCommand,
        prefix: &str,
    ) -> Result<()> {
        let subject = subjects::agent::sticker_set_mask_position(prefix);
        debug!("Setting mask position for sticker {}", cmd.sticker);

        let mut req = self.bot.set_sticker_mask_position(&cmd.sticker);
        if let Some(mp) = cmd.mask_position {
            req.mask_position = Some(convert_mask_position(mp));
        }
        if let Err(e) = req.await {
            self.handle_telegram_error(&subject, e, prefix).await?;
        }

        Ok(())
    }

    // ── Error handler ─────────────────────────────────────────────────────────

    /// Classify a Telegram API error and publish a CommandErrorEvent to NATS.
    ///
    /// - `Retry`    → logs a warning (actual re-execution is the caller's responsibility)
    /// - `Migrated` → logs the new chat ID
    /// - `Permanent` / `Transient` → publishes the event to `telegram.{prefix}.bot.error.command`
    /// Classify a Telegram API error, publish a `CommandErrorEvent` to NATS, and
    /// return the appropriate NAK signal to the dispatch loop:
    ///
    /// - `Retry(d)`   → `Err(NakError { delay: Some(d) })` — redeliver after rate-limit window
    /// - `Transient`  → `Err(NakError { delay: None })` — redeliver immediately
    /// - `Permanent` / `Migrated` → `Ok(())` — ack, do not redeliver
    pub(crate) async fn handle_telegram_error(
        &self,
        command_subject: &str,
        err: teloxide::RequestError,
        prefix: &str,
    ) -> anyhow::Result<()> {
        let outcome = classify(command_subject, &err);

        // Read the dispatch context populated by the run() loop
        let ctx = self.dispatch_ctx.read().await.clone();

        let evt = match &outcome {
            ErrorOutcome::Retry(duration) => {
                warn!(
                    "Rate limited on '{}': retry after {:?}",
                    command_subject, duration
                );
                telegram_types::errors::CommandErrorEvent {
                    command_subject: command_subject.to_string(),
                    error_code: telegram_types::errors::TelegramErrorCode::FloodControl,
                    category: telegram_types::errors::ErrorCategory::RateLimit,
                    message: format!("Rate limited: retry after {}s", duration.as_secs()),
                    retry_after_secs: Some(duration.as_secs()),
                    migrated_to_chat_id: None,
                    is_permanent: false,
                    chat_id: ctx.chat_id,
                    session_id: ctx.session_id.clone(),
                }
            }
            ErrorOutcome::Migrated(new_chat_id) => {
                warn!(
                    "Chat migrated on '{}': new chat_id={}",
                    command_subject, new_chat_id.0
                );
                telegram_types::errors::CommandErrorEvent {
                    command_subject: command_subject.to_string(),
                    error_code: telegram_types::errors::TelegramErrorCode::MigrateToChatId,
                    category: telegram_types::errors::ErrorCategory::ChatMigrated,
                    message: format!("Chat migrated to new ID: {}", new_chat_id.0),
                    retry_after_secs: None,
                    migrated_to_chat_id: Some(new_chat_id.0),
                    is_permanent: false,
                    chat_id: ctx.chat_id,
                    session_id: ctx.session_id.clone(),
                }
            }
            ErrorOutcome::Permanent(evt) | ErrorOutcome::Transient(evt) => {
                let mut evt = evt.clone();
                evt.chat_id = ctx.chat_id;
                evt.session_id = ctx.session_id.clone();
                evt
            }
        };

        let error_subject = subjects::bot::command_error(prefix);
        match serde_json::to_vec(&evt) {
            Ok(payload) => {
                if let Err(e) = self
                    .subscriber
                    .client()
                    .publish(error_subject.clone(), payload.into())
                    .await
                {
                    error!(
                        "Failed to publish command error to '{}': {}",
                        error_subject, e
                    );
                }
            }
            Err(e) => error!("Failed to serialize CommandErrorEvent: {}", e),
        }

        match outcome {
            ErrorOutcome::Retry(d) => Err(NakError { delay: Some(d) }.into()),
            ErrorOutcome::Transient(_) => Err(NakError { delay: None }.into()),
            ErrorOutcome::Permanent(_) | ErrorOutcome::Migrated(_) => Ok(()),
        }
    }

    // Stream message handling in outbound_streaming.rs module
}

/// Convert our ParseMode to Teloxide's ParseMode
#[allow(deprecated)]
pub(crate) fn convert_parse_mode(mode: ParseMode) -> TgParseMode {
    match mode {
        ParseMode::Markdown => TgParseMode::Markdown,
        ParseMode::MarkdownV2 => TgParseMode::MarkdownV2,
        ParseMode::HTML => TgParseMode::Html,
    }
}

/// Convert our ChatAction to Teloxide's ChatAction
pub(crate) fn convert_chat_action(action: ChatAction) -> teloxide::types::ChatAction {
    match action {
        ChatAction::Typing => teloxide::types::ChatAction::Typing,
        ChatAction::UploadPhoto => teloxide::types::ChatAction::UploadPhoto,
        ChatAction::RecordVideo => teloxide::types::ChatAction::RecordVideo,
        ChatAction::UploadVideo => teloxide::types::ChatAction::UploadVideo,
        ChatAction::RecordVoice => teloxide::types::ChatAction::RecordVoice,
        ChatAction::UploadVoice => teloxide::types::ChatAction::UploadVoice,
        ChatAction::UploadDocument => teloxide::types::ChatAction::UploadDocument,
        ChatAction::ChooseSticker => teloxide::types::ChatAction::Typing, // no direct equivalent in this teloxide version
        ChatAction::FindLocation => teloxide::types::ChatAction::FindLocation,
    }
}

/// Convert our InlineQueryResult to Teloxide's
fn convert_inline_query_result(
    result: telegram_types::commands::InlineQueryResult,
) -> Option<teloxide::types::InlineQueryResult> {
    use telegram_types::commands::InlineQueryResult as OurResult;
    use teloxide::types::{
        InlineQueryResult as TgResult, InlineQueryResultArticle, InlineQueryResultAudio,
        InlineQueryResultCachedAudio, InlineQueryResultCachedDocument, InlineQueryResultCachedGif,
        InlineQueryResultCachedMpeg4Gif, InlineQueryResultCachedPhoto,
        InlineQueryResultCachedSticker, InlineQueryResultCachedVideo, InlineQueryResultCachedVoice,
        InlineQueryResultContact, InlineQueryResultDocument, InlineQueryResultGame,
        InlineQueryResultGif, InlineQueryResultLocation, InlineQueryResultMpeg4Gif,
        InlineQueryResultPhoto, InlineQueryResultVenue, InlineQueryResultVideo,
        InlineQueryResultVoice, InputMessageContent, InputMessageContentText,
    };
    use url::Url;

    match result {
        OurResult::Article(article) => {
            let input_content = InputMessageContent::Text(InputMessageContentText::new(
                article.input_message_content.message_text,
            ));

            let mut result =
                InlineQueryResultArticle::new(article.id, article.title, input_content);

            if let Some(desc) = article.description {
                result.description = Some(desc);
            }

            Some(TgResult::Article(result))
        }

        OurResult::Photo(photo) => {
            let photo_url = match Url::parse(&photo.photo_url) {
                Ok(url) => url,
                Err(e) => {
                    warn!("Invalid photo URL '{}': {}", photo.photo_url, e);
                    return None;
                }
            };

            let thumb_url = match Url::parse(&photo.thumb_url) {
                Ok(url) => url,
                Err(e) => {
                    warn!("Invalid thumb URL '{}': {}", photo.thumb_url, e);
                    return None;
                }
            };

            let mut result = InlineQueryResultPhoto::new(photo.id, photo_url, thumb_url);
            result.title = photo.title;
            result.description = photo.description;
            result.caption = photo.caption;

            Some(TgResult::Photo(result))
        }

        OurResult::CachedSticker(sticker) => {
            let mut result =
                InlineQueryResultCachedSticker::new(sticker.id, sticker.sticker_file_id);

            if let Some(markup) = sticker.reply_markup {
                result.reply_markup = Some(convert_inline_keyboard(markup));
            }

            Some(TgResult::CachedSticker(result))
        }

        OurResult::Gif(gif) => {
            let gif_url = match Url::parse(&gif.gif_url) {
                Ok(url) => url,
                Err(e) => {
                    warn!("Invalid GIF URL '{}': {}", gif.gif_url, e);
                    return None;
                }
            };

            let thumb_url = match Url::parse(&gif.thumbnail_url) {
                Ok(url) => url,
                Err(e) => {
                    warn!("Invalid thumbnail URL '{}': {}", gif.thumbnail_url, e);
                    return None;
                }
            };

            let mut result = InlineQueryResultGif::new(gif.id, gif_url, thumb_url);

            result.gif_width = gif.gif_width;
            result.gif_height = gif.gif_height;
            result.gif_duration = gif.gif_duration.map(teloxide::types::Seconds::from_seconds);
            result.title = gif.title;
            result.caption = gif.caption;
            result.parse_mode = gif.parse_mode.map(convert_parse_mode);

            Some(TgResult::Gif(result))
        }

        OurResult::CachedGif(gif) => {
            let mut result = InlineQueryResultCachedGif::new(gif.id, gif.gif_file_id);

            result.title = gif.title;
            result.caption = gif.caption;
            result.parse_mode = gif.parse_mode.map(convert_parse_mode);

            Some(TgResult::CachedGif(result))
        }

        OurResult::Mpeg4Gif(mp4) => {
            let mp4_url = match Url::parse(&mp4.mpeg4_url) {
                Ok(url) => url,
                Err(e) => {
                    warn!("Invalid MP4 URL '{}': {}", mp4.mpeg4_url, e);
                    return None;
                }
            };

            let thumb_url = match Url::parse(&mp4.thumbnail_url) {
                Ok(url) => url,
                Err(e) => {
                    warn!("Invalid thumbnail URL '{}': {}", mp4.thumbnail_url, e);
                    return None;
                }
            };

            let mut result = InlineQueryResultMpeg4Gif::new(mp4.id, mp4_url, thumb_url);

            result.mpeg4_width = mp4.mpeg4_width;
            result.mpeg4_height = mp4.mpeg4_height;
            result.mpeg4_duration = mp4
                .mpeg4_duration
                .map(teloxide::types::Seconds::from_seconds);
            result.title = mp4.title;
            result.caption = mp4.caption;
            result.parse_mode = mp4.parse_mode.map(convert_parse_mode);

            Some(TgResult::Mpeg4Gif(result))
        }

        OurResult::CachedMpeg4Gif(mp4) => {
            let mut result = InlineQueryResultCachedMpeg4Gif::new(mp4.id, mp4.mpeg4_file_id);

            result.title = mp4.title;
            result.caption = mp4.caption;
            result.parse_mode = mp4.parse_mode.map(convert_parse_mode);

            Some(TgResult::CachedMpeg4Gif(result))
        }

        OurResult::Video(video) => {
            let video_url = match Url::parse(&video.video_url) {
                Ok(url) => url,
                Err(e) => {
                    warn!("Invalid video URL '{}': {}", video.video_url, e);
                    return None;
                }
            };

            let thumb_url = match Url::parse(&video.thumbnail_url) {
                Ok(url) => url,
                Err(e) => {
                    warn!("Invalid thumbnail URL '{}': {}", video.thumbnail_url, e);
                    return None;
                }
            };

            let mime_type = match video.mime_type.parse::<mime::Mime>() {
                Ok(mime) => mime,
                Err(e) => {
                    warn!("Invalid MIME type '{}': {}", video.mime_type, e);
                    return None;
                }
            };

            let mut result =
                InlineQueryResultVideo::new(video.id, video_url, mime_type, thumb_url, video.title);

            result.caption = video.caption;
            result.parse_mode = video.parse_mode.map(convert_parse_mode);
            result.video_width = video.video_width;
            result.video_height = video.video_height;
            result.video_duration = video
                .video_duration
                .map(teloxide::types::Seconds::from_seconds);
            result.description = video.description;

            Some(TgResult::Video(result))
        }

        OurResult::CachedVideo(video) => {
            let mut result =
                InlineQueryResultCachedVideo::new(video.id, video.video_file_id, video.title);

            result.description = video.description;
            result.caption = video.caption;
            result.parse_mode = video.parse_mode.map(convert_parse_mode);

            Some(TgResult::CachedVideo(result))
        }

        OurResult::Audio(audio) => {
            let audio_url = match Url::parse(&audio.audio_url) {
                Ok(url) => url,
                Err(e) => {
                    warn!("Invalid audio URL '{}': {}", audio.audio_url, e);
                    return None;
                }
            };

            let mut result = InlineQueryResultAudio::new(audio.id, audio_url, audio.title);

            result.caption = audio.caption;
            result.parse_mode = audio.parse_mode.map(convert_parse_mode);
            result.performer = audio.performer;
            result.audio_duration = audio
                .audio_duration
                .map(teloxide::types::Seconds::from_seconds);

            Some(TgResult::Audio(result))
        }

        OurResult::CachedAudio(audio) => {
            let mut result = InlineQueryResultCachedAudio::new(audio.id, audio.audio_file_id);

            result.caption = audio.caption;
            result.parse_mode = audio.parse_mode.map(convert_parse_mode);

            Some(TgResult::CachedAudio(result))
        }

        OurResult::Voice(voice) => {
            let voice_url = match Url::parse(&voice.voice_url) {
                Ok(url) => url,
                Err(e) => {
                    warn!("Invalid voice URL '{}': {}", voice.voice_url, e);
                    return None;
                }
            };

            let mut result = InlineQueryResultVoice::new(voice.id, voice_url, voice.title);

            result.caption = voice.caption;
            result.parse_mode = voice.parse_mode.map(convert_parse_mode);
            result.voice_duration = voice
                .voice_duration
                .map(teloxide::types::Seconds::from_seconds);

            Some(TgResult::Voice(result))
        }

        OurResult::CachedVoice(voice) => {
            let mut result =
                InlineQueryResultCachedVoice::new(voice.id, voice.voice_file_id, voice.title);

            result.caption = voice.caption;
            result.parse_mode = voice.parse_mode.map(convert_parse_mode);

            Some(TgResult::CachedVoice(result))
        }

        OurResult::Document(document) => {
            let document_url = match Url::parse(&document.document_url) {
                Ok(url) => url,
                Err(e) => {
                    warn!("Invalid document URL '{}': {}", document.document_url, e);
                    return None;
                }
            };

            let mime_type = match document.mime_type.parse::<mime::Mime>() {
                Ok(mime) => mime,
                Err(e) => {
                    warn!("Invalid MIME type '{}': {}", document.mime_type, e);
                    return None;
                }
            };

            let thumbnail_url = document.thumbnail_url.and_then(|s| Url::parse(&s).ok());

            let result = InlineQueryResultDocument {
                id: document.id,
                title: document.title,
                document_url,
                mime_type,
                caption: document.caption,
                parse_mode: document.parse_mode.map(convert_parse_mode),
                caption_entities: None,
                description: document.description,
                reply_markup: None,
                input_message_content: None,
                thumbnail_url,
                thumbnail_width: document.thumbnail_width,
                thumbnail_height: document.thumbnail_height,
            };

            Some(TgResult::Document(result))
        }

        OurResult::CachedDocument(document) => {
            let mut result = InlineQueryResultCachedDocument::new(
                document.id,
                document.title,
                document.document_file_id,
            );

            result.description = document.description;
            result.caption = document.caption;
            result.parse_mode = document.parse_mode.map(convert_parse_mode);

            Some(TgResult::CachedDocument(result))
        }

        OurResult::CachedPhoto(photo) => {
            let mut result = InlineQueryResultCachedPhoto::new(photo.id, photo.photo_file_id);

            result.title = photo.title;
            result.description = photo.description;
            result.caption = photo.caption;
            result.parse_mode = photo.parse_mode.map(convert_parse_mode);

            Some(TgResult::CachedPhoto(result))
        }

        OurResult::Location(location) => {
            let mut result = InlineQueryResultLocation::new(
                location.id,
                location.title,
                location.latitude,
                location.longitude,
            );

            result.horizontal_accuracy = location.horizontal_accuracy;
            result.live_period = location.live_period.map(|s| {
                teloxide::types::LivePeriod::from_seconds(teloxide::types::Seconds::from_seconds(s))
            });
            result.heading = location.heading.map(|h| h as u16);
            result.proximity_alert_radius = location.proximity_alert_radius;

            if let Some(thumb_url_str) = location.thumbnail_url {
                if let Ok(thumb_url) = Url::parse(&thumb_url_str) {
                    result.thumbnail_url = Some(thumb_url);
                }
            }

            result.thumbnail_width = location.thumbnail_width;
            result.thumbnail_height = location.thumbnail_height;

            Some(TgResult::Location(result))
        }

        OurResult::Venue(venue) => {
            let mut result = InlineQueryResultVenue::new(
                venue.id,
                venue.latitude,
                venue.longitude,
                venue.title,
                venue.address,
            );

            result.foursquare_id = venue.foursquare_id;
            result.foursquare_type = venue.foursquare_type;
            result.google_place_id = venue.google_place_id;
            result.google_place_type = venue.google_place_type;

            if let Some(thumb_url_str) = venue.thumbnail_url {
                if let Ok(thumb_url) = Url::parse(&thumb_url_str) {
                    result.thumbnail_url = Some(thumb_url);
                }
            }

            result.thumbnail_width = venue.thumbnail_width;
            result.thumbnail_height = venue.thumbnail_height;

            Some(TgResult::Venue(result))
        }

        OurResult::Contact(contact) => {
            let mut result =
                InlineQueryResultContact::new(contact.id, contact.phone_number, contact.first_name);

            result.last_name = contact.last_name;
            result.vcard = contact.vcard;

            if let Some(thumb_url_str) = contact.thumbnail_url {
                if let Ok(thumb_url) = Url::parse(&thumb_url_str) {
                    result.thumbnail_url = Some(thumb_url);
                }
            }

            result.thumbnail_width = contact.thumbnail_width;
            result.thumbnail_height = contact.thumbnail_height;

            Some(TgResult::Contact(result))
        }

        OurResult::Game(game) => {
            let result = InlineQueryResultGame::new(game.id, game.game_short_name);

            Some(TgResult::Game(result))
        }
    }
}

/// Convert our InlineKeyboardMarkup to Teloxide's
fn convert_inline_keyboard(
    markup: telegram_types::chat::InlineKeyboardMarkup,
) -> teloxide::types::InlineKeyboardMarkup {
    use teloxide::types::{InlineKeyboardButton, InlineKeyboardButtonKind};

    let buttons: Vec<Vec<InlineKeyboardButton>> = markup
        .inline_keyboard
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|btn| {
                    let kind = if let Some(data) = btn.callback_data {
                        InlineKeyboardButtonKind::CallbackData(data)
                    } else if let Some(url) = btn.url {
                        InlineKeyboardButtonKind::Url(
                            url.parse()
                                .unwrap_or_else(|_| "https://example.com".parse().unwrap()),
                        )
                    } else {
                        // Default to callback with empty data
                        InlineKeyboardButtonKind::CallbackData(String::new())
                    };
                    InlineKeyboardButton::new(btn.text, kind)
                })
                .collect()
        })
        .collect();

    teloxide::types::InlineKeyboardMarkup::new(buttons)
}

/// Convert our ChatPermissions to Teloxide's ChatPermissions
fn convert_chat_permissions(
    perms: &telegram_types::chat::ChatPermissions,
) -> teloxide::types::ChatPermissions {
    use teloxide::types::ChatPermissions as CP;

    let mut result = CP::empty();

    if perms.can_send_messages.unwrap_or(false) {
        result |= CP::SEND_MESSAGES;
    }
    if perms.can_send_media_messages.unwrap_or(false) {
        result |= CP::SEND_AUDIOS
            | CP::SEND_DOCUMENTS
            | CP::SEND_PHOTOS
            | CP::SEND_VIDEOS
            | CP::SEND_VIDEO_NOTES
            | CP::SEND_VOICE_NOTES;
    }
    if perms.can_send_polls.unwrap_or(false) {
        result |= CP::SEND_POLLS;
    }
    if perms.can_send_other_messages.unwrap_or(false) {
        result |= CP::SEND_OTHER_MESSAGES;
    }
    if perms.can_add_web_page_previews.unwrap_or(false) {
        result |= CP::ADD_WEB_PAGE_PREVIEWS;
    }
    if perms.can_change_info.unwrap_or(false) {
        result |= CP::CHANGE_INFO;
    }
    if perms.can_invite_users.unwrap_or(false) {
        result |= CP::INVITE_USERS;
    }
    if perms.can_pin_messages.unwrap_or(false) {
        result |= CP::PIN_MESSAGES;
    }
    if perms.can_manage_topics.unwrap_or(false) {
        result |= CP::MANAGE_TOPICS;
    }

    result
}

/// Convert our BotCommandScope to Teloxide's BotCommandScope
fn convert_bot_command_scope(
    scope: telegram_types::chat::BotCommandScope,
) -> teloxide::types::BotCommandScope {
    match scope {
        telegram_types::chat::BotCommandScope::Default => teloxide::types::BotCommandScope::Default,
        telegram_types::chat::BotCommandScope::AllPrivateChats => {
            teloxide::types::BotCommandScope::AllPrivateChats
        }
        telegram_types::chat::BotCommandScope::AllGroupChats => {
            teloxide::types::BotCommandScope::AllGroupChats
        }
        telegram_types::chat::BotCommandScope::AllChatAdministrators => {
            teloxide::types::BotCommandScope::AllChatAdministrators
        }
        telegram_types::chat::BotCommandScope::Chat { chat_id } => {
            teloxide::types::BotCommandScope::Chat {
                chat_id: teloxide::types::Recipient::Id(teloxide::types::ChatId(chat_id)),
            }
        }
        telegram_types::chat::BotCommandScope::ChatAdministrators { chat_id } => {
            teloxide::types::BotCommandScope::ChatAdministrators {
                chat_id: teloxide::types::Recipient::Id(teloxide::types::ChatId(chat_id)),
            }
        }
        telegram_types::chat::BotCommandScope::ChatMember { chat_id, user_id } => {
            teloxide::types::BotCommandScope::ChatMember {
                chat_id: teloxide::types::Recipient::Id(teloxide::types::ChatId(chat_id)),
                user_id: teloxide::types::UserId(user_id as u64),
            }
        }
    }
}

/// Convert our sticker format string to Teloxide's StickerFormat
fn convert_sticker_format(format: &str) -> teloxide::types::StickerFormat {
    match format {
        "animated" => teloxide::types::StickerFormat::Animated,
        "video" => teloxide::types::StickerFormat::Video,
        _ => teloxide::types::StickerFormat::Static,
    }
}

/// Convert our sticker type string to Teloxide's StickerType
fn convert_sticker_type(kind: &str) -> teloxide::types::StickerType {
    match kind {
        "mask" => teloxide::types::StickerType::Mask,
        "custom_emoji" => teloxide::types::StickerType::CustomEmoji,
        _ => teloxide::types::StickerType::Regular,
    }
}

/// Convert our InputSticker to Teloxide's InputSticker
fn convert_input_sticker(s: &telegram_types::chat::InputSticker) -> teloxide::types::InputSticker {
    use teloxide::types::InputSticker;

    let sticker_file = teloxide::types::InputFile::file_id(&s.sticker);
    let format = convert_sticker_format(&s.format);
    let mask_position = s
        .mask_position
        .as_ref()
        .map(|mp| convert_mask_position(mp.clone()));

    InputSticker {
        sticker: sticker_file,
        format,
        emoji_list: s.emoji_list.clone(),
        mask_position,
        keywords: s.keywords.clone(),
    }
}

/// Convert our MaskPosition to Teloxide's MaskPosition
fn convert_mask_position(mp: telegram_types::chat::MaskPosition) -> teloxide::types::MaskPosition {
    use teloxide::types::{MaskPoint, MaskPosition};

    let point = match mp.point {
        telegram_types::chat::MaskPoint::Forehead => MaskPoint::Forehead,
        telegram_types::chat::MaskPoint::Eyes => MaskPoint::Eyes,
        telegram_types::chat::MaskPoint::Mouth => MaskPoint::Mouth,
        telegram_types::chat::MaskPoint::Chin => MaskPoint::Chin,
    };

    MaskPosition::new(point, mp.x_shift, mp.y_shift, mp.scale)
}

/// Download a file from URL to local path
async fn download_file_from_url(url: &str, destination_path: &str) -> Result<u64> {
    use tokio::fs::File;
    use tokio::io::AsyncWriteExt;

    // Create parent directories if they don't exist
    if let Some(parent) = std::path::Path::new(destination_path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Download file
    let response = reqwest::get(url).await?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "Failed to download file: HTTP {}",
            response.status()
        ));
    }

    let bytes = response.bytes().await?;
    let file_size = bytes.len() as u64;

    // Write to file
    let mut file = File::create(destination_path).await?;
    file.write_all(&bytes).await?;
    file.flush().await?;

    Ok(file_size)
}
