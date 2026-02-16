//! Outbound message processor (NATS → Telegram)

use async_nats::Client;
use teloxide::{Bot, requests::Requester};
use teloxide::types::{ChatId, MessageId, ParseMode as TgParseMode};
use telegram_nats::{MessageSubscriber, subjects};
use telegram_types::commands::*;
use tracing::{debug, error, info, warn};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::Instant;

/// Tracking info for streaming messages
#[derive(Debug, Clone)]
pub(crate) struct StreamingMessage {
    pub message_id: i32,
    pub last_edit: Instant,
    pub edit_count: u32,
}

/// Outbound processor that listens to agent commands and executes them
pub struct OutboundProcessor {
    pub(crate) bot: Bot,
    pub(crate) subscriber: MessageSubscriber,
    /// Track streaming messages by (chat_id, session_id)
    pub(crate) streaming_messages: Arc<RwLock<HashMap<(i64, String), StreamingMessage>>>,
}

impl OutboundProcessor {
    /// Create a new outbound processor
    pub fn new(bot: Bot, client: Client, prefix: String) -> Self {
        Self {
            bot,
            subscriber: MessageSubscriber::new(client, prefix),
            streaming_messages: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Run the outbound processor
    pub async fn run(self) -> Result<()> {
        info!("Starting outbound processor");

        // Subscribe to all agent commands
        let prefix = self.subscriber.prefix().to_string();

        // Spawn tasks for each command type
        let send_task = self.handle_send_messages(prefix.clone());
        let edit_task = self.handle_edit_messages(prefix.clone());
        let delete_task = self.handle_delete_messages(prefix.clone());
        let photo_task = self.handle_send_photos(prefix.clone());
        let callback_task = self.handle_answer_callbacks(prefix.clone());
        let action_task = self.handle_chat_actions(prefix.clone());
        let stream_task = self.handle_stream_messages(prefix.clone());

        // Run all tasks concurrently
        tokio::try_join!(
            send_task,
            edit_task,
            delete_task,
            photo_task,
            callback_task,
            action_task,
            stream_task
        )?;

        Ok(())
    }

    /// Handle send message commands
    async fn handle_send_messages(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_send(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self.subscriber.subscribe::<SendMessageCommand>(&subject).await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    debug!("Received send message command for chat {}", cmd.chat_id);

                    let mut req = self.bot.send_message(ChatId(cmd.chat_id), cmd.text);

                    if let Some(parse_mode) = cmd.parse_mode {
                        req.parse_mode = Some(convert_parse_mode(parse_mode));
                    }

                    if let Some(reply_to) = cmd.reply_to_message_id {
                        req.reply_parameters = Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
                    }

                    if let Some(markup) = cmd.reply_markup {
                        req.reply_markup = Some(teloxide::types::ReplyMarkup::InlineKeyboard(convert_inline_keyboard(markup)));
                    }

                    if let Err(e) = req.await {
                        error!("Failed to send message: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize send message command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle edit message commands
    async fn handle_edit_messages(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_edit(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self.subscriber.subscribe::<EditMessageCommand>(&subject).await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    debug!("Received edit message command for chat {} msg {}", cmd.chat_id, cmd.message_id);

                    let mut req = self.bot.edit_message_text(
                        ChatId(cmd.chat_id),
                        MessageId(cmd.message_id),
                        cmd.text
                    );

                    if let Some(parse_mode) = cmd.parse_mode {
                        req.parse_mode = Some(convert_parse_mode(parse_mode));
                    }

                    if let Some(markup) = cmd.reply_markup {
                        req.reply_markup = Some(convert_inline_keyboard(markup));
                    }

                    if let Err(e) = req.await {
                        error!("Failed to edit message: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize edit message command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle delete message commands
    async fn handle_delete_messages(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_delete(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self.subscriber.subscribe::<DeleteMessageCommand>(&subject).await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    debug!("Received delete message command for chat {} msg {}", cmd.chat_id, cmd.message_id);

                    if let Err(e) = self.bot.delete_message(ChatId(cmd.chat_id), MessageId(cmd.message_id)).await {
                        error!("Failed to delete message: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize delete message command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle send photo commands
    async fn handle_send_photos(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_send_photo(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self.subscriber.subscribe::<SendPhotoCommand>(&subject).await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
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

                    if let Err(e) = req.await {
                        error!("Failed to send photo: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize send photo command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle answer callback commands
    async fn handle_answer_callbacks(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::callback_answer(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self.subscriber.subscribe::<AnswerCallbackCommand>(&subject).await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    debug!("Received answer callback command");

                    let mut req = self.bot.answer_callback_query(&cmd.callback_query_id);

                    if let Some(text) = cmd.text {
                        req.text = Some(text);
                    }

                    if let Some(show_alert) = cmd.show_alert {
                        req.show_alert = Some(show_alert);
                    }

                    if let Err(e) = req.await {
                        error!("Failed to answer callback: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize answer callback command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle chat action commands
    async fn handle_chat_actions(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::chat_action(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self.subscriber.subscribe::<SendChatActionCommand>(&subject).await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    debug!("Received chat action command for chat {}", cmd.chat_id);

                    let action = convert_chat_action(cmd.action);
                    if let Err(e) = self.bot.send_chat_action(ChatId(cmd.chat_id), action).await {
                        warn!("Failed to send chat action: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize chat action command: {}", e),
            }
        }

        Ok(())
    }

    // Stream message handling moved to outbound_streaming.rs module
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
fn convert_chat_action(action: ChatAction) -> teloxide::types::ChatAction {
    match action {
        ChatAction::Typing => teloxide::types::ChatAction::Typing,
        ChatAction::UploadPhoto => teloxide::types::ChatAction::UploadPhoto,
        ChatAction::RecordVideo => teloxide::types::ChatAction::RecordVideo,
        ChatAction::UploadVideo => teloxide::types::ChatAction::UploadVideo,
        ChatAction::RecordVoice => teloxide::types::ChatAction::RecordVoice,
        ChatAction::UploadVoice => teloxide::types::ChatAction::UploadVoice,
        ChatAction::UploadDocument => teloxide::types::ChatAction::UploadDocument,
        ChatAction::ChooseSticker => teloxide::types::ChatAction::Typing, // Fallback to Typing
        ChatAction::FindLocation => teloxide::types::ChatAction::FindLocation,
    }
}

/// Convert our InlineKeyboardMarkup to Teloxide's
fn convert_inline_keyboard(markup: telegram_types::chat::InlineKeyboardMarkup) -> teloxide::types::InlineKeyboardMarkup {
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
                        InlineKeyboardButtonKind::Url(url.parse().unwrap_or_else(|_| "https://example.com".parse().unwrap()))
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
