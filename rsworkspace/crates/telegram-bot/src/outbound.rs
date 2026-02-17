//! Outbound message processor (NATS → Telegram)

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
        let poll_task = self.handle_polls(prefix.clone());
        let photo_task = self.handle_send_photos(prefix.clone());
        let video_task = self.handle_send_videos(prefix.clone());
        let audio_task = self.handle_send_audios(prefix.clone());
        let document_task = self.handle_send_documents(prefix.clone());
        let voice_task = self.handle_send_voices(prefix.clone());
        let media_group_task = self.handle_send_media_groups(prefix.clone());
        let sticker_task = self.handle_send_stickers(prefix.clone());
        let animation_task = self.handle_send_animations(prefix.clone());
        let video_note_task = self.handle_send_video_notes(prefix.clone());
        let location_task = self.handle_send_locations(prefix.clone());
        let venue_task = self.handle_send_venues(prefix.clone());
        let contact_task = self.handle_send_contacts(prefix.clone());
        let callback_task = self.handle_answer_callbacks(prefix.clone());
        let action_task = self.handle_chat_actions(prefix.clone());
        let stream_task = self.handle_stream_messages(prefix.clone());
        let inline_task = self.handle_answer_inline_queries(prefix.clone());
        let forum_task = self.handle_forum_topics(prefix.clone());
        let admin_task = self.handle_admin_commands(prefix.clone());
        let file_task = self.handle_file_commands(prefix.clone());
        let payment_task = self.handle_payment_commands(prefix.clone());
        let bot_commands_task = self.handle_bot_commands(prefix.clone());
        let sticker_mgmt_task = self.handle_sticker_management(prefix.clone());
        let forward_task = self.handle_forward_messages(prefix.clone());
        let copy_task = self.handle_copy_messages(prefix.clone());

        // Run all tasks concurrently
        tokio::try_join!(
            send_task,
            edit_task,
            delete_task,
            poll_task,
            photo_task,
            video_task,
            audio_task,
            document_task,
            voice_task,
            media_group_task,
            sticker_task,
            animation_task,
            video_note_task,
            location_task,
            venue_task,
            contact_task,
            callback_task,
            action_task,
            stream_task,
            inline_task,
            forum_task,
            admin_task,
            file_task,
            payment_task,
            bot_commands_task,
            sticker_mgmt_task,
            forward_task,
            copy_task
        )?;

        Ok(())
    }

    /// Handle send poll and stop poll commands
    async fn handle_polls(&self, prefix: String) -> Result<()> {
        use telegram_types::commands::PollKind;
        use teloxide::types::PollType as TgPollType;

        let send_subject = subjects::agent::poll_send(&prefix);
        let stop_subject = subjects::agent::poll_stop(&prefix);

        info!("Subscribing to {} and {}", send_subject, stop_subject);

        let mut send_stream = self
            .subscriber
            .subscribe::<SendPollCommand>(&send_subject)
            .await?;
        let mut stop_stream = self
            .subscriber
            .subscribe::<StopPollCommand>(&stop_subject)
            .await?;

        loop {
            tokio::select! {
                Some(result) = send_stream.next() => {
                    match result {
                        Ok(cmd) => {
                            debug!("Received send poll command for chat {}", cmd.chat_id);

                            let options: Vec<teloxide::types::InputPollOption> = cmd.options
                                .into_iter()
                                .map(teloxide::types::InputPollOption::new)
                                .collect();

                            let poll_type = match cmd.poll_type {
                                Some(PollKind::Quiz) => TgPollType::Quiz,
                                _ => TgPollType::Regular,
                            };

                            let mut req = self.bot.send_poll(
                                ChatId(cmd.chat_id),
                                cmd.question,
                                options,
                            );

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
                                self.handle_telegram_error(&send_subject, e, &prefix).await;
                            }
                        }
                        Err(e) => error!("Failed to deserialize send poll command: {}", e),
                    }
                }
                Some(result) = stop_stream.next() => {
                    match result {
                        Ok(cmd) => {
                            debug!("Received stop poll command for chat {}", cmd.chat_id);

                            let mut req = self.bot.stop_poll(ChatId(cmd.chat_id), MessageId(cmd.message_id));

                            if let Some(markup) = cmd.reply_markup {
                                req.reply_markup = Some(convert_inline_keyboard(markup));
                            }

                            if let Err(e) = req.await {
                                self.handle_telegram_error(&stop_subject, e, &prefix).await;
                            }
                        }
                        Err(e) => error!("Failed to deserialize stop poll command: {}", e),
                    }
                }
                else => break,
            }
        }

        Ok(())
    }

    /// Handle send message commands
    async fn handle_send_messages(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_send(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self
            .subscriber
            .subscribe::<SendMessageCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    debug!("Received send message command for chat {}", cmd.chat_id);

                    let mut req = self.bot.send_message(ChatId(cmd.chat_id), cmd.text);

                    if let Some(parse_mode) = cmd.parse_mode {
                        req.parse_mode = Some(convert_parse_mode(parse_mode));
                    }

                    if let Some(reply_to) = cmd.reply_to_message_id {
                        req.reply_parameters =
                            Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
                    }

                    if let Some(markup) = cmd.reply_markup {
                        req.reply_markup = Some(teloxide::types::ReplyMarkup::InlineKeyboard(
                            convert_inline_keyboard(markup),
                        ));
                    }

                    if let Some(thread_id) = cmd.message_thread_id {
                        req.message_thread_id =
                            Some(teloxide::types::ThreadId(MessageId(thread_id)));
                    }

                    if let Err(e) = req.await {
                        self.handle_telegram_error(&subject, e, &prefix).await;
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

        let mut stream = self
            .subscriber
            .subscribe::<EditMessageCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    debug!(
                        "Received edit message command for chat {} msg {}",
                        cmd.chat_id, cmd.message_id
                    );

                    let mut req = self.bot.edit_message_text(
                        ChatId(cmd.chat_id),
                        MessageId(cmd.message_id),
                        cmd.text,
                    );

                    if let Some(parse_mode) = cmd.parse_mode {
                        req.parse_mode = Some(convert_parse_mode(parse_mode));
                    }

                    if let Some(markup) = cmd.reply_markup {
                        req.reply_markup = Some(convert_inline_keyboard(markup));
                    }

                    if let Err(e) = req.await {
                        self.handle_telegram_error(&subject, e, &prefix).await;
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

        let mut stream = self
            .subscriber
            .subscribe::<DeleteMessageCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    debug!(
                        "Received delete message command for chat {} msg {}",
                        cmd.chat_id, cmd.message_id
                    );

                    if let Err(e) = self
                        .bot
                        .delete_message(ChatId(cmd.chat_id), MessageId(cmd.message_id))
                        .await
                    {
                        self.handle_telegram_error(&subject, e, &prefix).await;
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

        let mut stream = self
            .subscriber
            .subscribe::<SendPhotoCommand>(&subject)
            .await?;

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
                        req.reply_parameters =
                            Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
                    }

                    if let Some(thread_id) = cmd.message_thread_id {
                        req.message_thread_id =
                            Some(teloxide::types::ThreadId(MessageId(thread_id)));
                    }

                    if let Err(e) = req.await {
                        self.handle_telegram_error(&subject, e, &prefix).await;
                    }
                }
                Err(e) => error!("Failed to deserialize send photo command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle send video commands
    async fn handle_send_videos(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_send_video(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self
            .subscriber
            .subscribe::<SendVideoCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
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
                        req.reply_parameters =
                            Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
                    }

                    if let Some(thread_id) = cmd.message_thread_id {
                        req.message_thread_id =
                            Some(teloxide::types::ThreadId(MessageId(thread_id)));
                    }

                    if let Err(e) = req.await {
                        self.handle_telegram_error(&subject, e, &prefix).await;
                    }
                }
                Err(e) => error!("Failed to deserialize send video command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle send audio commands
    async fn handle_send_audios(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_send_audio(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self
            .subscriber
            .subscribe::<SendAudioCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
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
                        req.reply_parameters =
                            Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
                    }

                    if let Some(thread_id) = cmd.message_thread_id {
                        req.message_thread_id =
                            Some(teloxide::types::ThreadId(MessageId(thread_id)));
                    }

                    if let Err(e) = req.await {
                        self.handle_telegram_error(&subject, e, &prefix).await;
                    }
                }
                Err(e) => error!("Failed to deserialize send audio command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle send document commands
    async fn handle_send_documents(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_send_document(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self
            .subscriber
            .subscribe::<SendDocumentCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
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
                        req.reply_parameters =
                            Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
                    }

                    if let Some(thread_id) = cmd.message_thread_id {
                        req.message_thread_id =
                            Some(teloxide::types::ThreadId(MessageId(thread_id)));
                    }

                    if let Err(e) = req.await {
                        self.handle_telegram_error(&subject, e, &prefix).await;
                    }
                }
                Err(e) => error!("Failed to deserialize send document command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle send voice commands
    async fn handle_send_voices(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_send_voice(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self
            .subscriber
            .subscribe::<SendVoiceCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
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
                        req.reply_parameters =
                            Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
                    }

                    if let Some(thread_id) = cmd.message_thread_id {
                        req.message_thread_id =
                            Some(teloxide::types::ThreadId(MessageId(thread_id)));
                    }

                    if let Err(e) = req.await {
                        self.handle_telegram_error(&subject, e, &prefix).await;
                    }
                }
                Err(e) => error!("Failed to deserialize send voice command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle send media group commands
    async fn handle_send_media_groups(&self, prefix: String) -> Result<()> {
        use telegram_types::commands::InputMediaItem;
        use teloxide::types::{
            InputMedia, InputMediaAudio, InputMediaDocument, InputMediaPhoto, InputMediaVideo,
        };

        let subject = subjects::agent::message_send_media_group(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self
            .subscriber
            .subscribe::<SendMediaGroupCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
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
                                let mut m = InputMediaPhoto::new(
                                    teloxide::types::InputFile::file_id(media),
                                );
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
                                let mut m = InputMediaVideo::new(
                                    teloxide::types::InputFile::file_id(media),
                                );
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
                                let mut m = InputMediaAudio::new(
                                    teloxide::types::InputFile::file_id(media),
                                );
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
                                let mut m = InputMediaDocument::new(
                                    teloxide::types::InputFile::file_id(media),
                                );
                                m.caption = caption;
                                m.parse_mode = parse_mode.map(convert_parse_mode);
                                InputMedia::Document(m)
                            }
                        })
                        .collect();

                    let mut req = self.bot.send_media_group(ChatId(cmd.chat_id), media);

                    if let Some(reply_to) = cmd.reply_to_message_id {
                        req.reply_parameters =
                            Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
                    }

                    if let Some(thread_id) = cmd.message_thread_id {
                        req.message_thread_id =
                            Some(teloxide::types::ThreadId(MessageId(thread_id)));
                    }

                    if let Err(e) = req.await {
                        self.handle_telegram_error(&subject, e, &prefix).await;
                    }
                }
                Err(e) => error!("Failed to deserialize send media group command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle send sticker commands
    async fn handle_send_stickers(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_send_sticker(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self
            .subscriber
            .subscribe::<SendStickerCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    debug!("Received send sticker command for chat {}", cmd.chat_id);

                    let sticker = teloxide::types::InputFile::file_id(cmd.sticker);
                    let mut req = self.bot.send_sticker(ChatId(cmd.chat_id), sticker);

                    if let Some(reply_to) = cmd.reply_to_message_id {
                        req.reply_parameters =
                            Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
                    }

                    if let Some(markup) = cmd.reply_markup {
                        req.reply_markup = Some(teloxide::types::ReplyMarkup::InlineKeyboard(
                            convert_inline_keyboard(markup),
                        ));
                    }

                    if let Some(thread_id) = cmd.message_thread_id {
                        req.message_thread_id =
                            Some(teloxide::types::ThreadId(MessageId(thread_id)));
                    }

                    if let Err(e) = req.await {
                        self.handle_telegram_error(&subject, e, &prefix).await;
                    }
                }
                Err(e) => error!("Failed to deserialize send sticker command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle send animation (GIF) commands
    async fn handle_send_animations(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_send_animation(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self
            .subscriber
            .subscribe::<SendAnimationCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
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
                        req.reply_parameters =
                            Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
                    }

                    if let Some(markup) = cmd.reply_markup {
                        req.reply_markup = Some(teloxide::types::ReplyMarkup::InlineKeyboard(
                            convert_inline_keyboard(markup),
                        ));
                    }

                    if let Some(thread_id) = cmd.message_thread_id {
                        req.message_thread_id =
                            Some(teloxide::types::ThreadId(MessageId(thread_id)));
                    }

                    if let Err(e) = req.await {
                        self.handle_telegram_error(&subject, e, &prefix).await;
                    }
                }
                Err(e) => error!("Failed to deserialize send animation command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle send video note commands
    async fn handle_send_video_notes(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_send_video_note(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self
            .subscriber
            .subscribe::<SendVideoNoteCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
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
                        req.reply_parameters =
                            Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
                    }

                    if let Some(thread_id) = cmd.message_thread_id {
                        req.message_thread_id =
                            Some(teloxide::types::ThreadId(MessageId(thread_id)));
                    }

                    if let Err(e) = req.await {
                        self.handle_telegram_error(&subject, e, &prefix).await;
                    }
                }
                Err(e) => error!("Failed to deserialize send video note command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle send location commands
    async fn handle_send_locations(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_send_location(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self
            .subscriber
            .subscribe::<SendLocationCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    debug!("Received send location command for chat {}", cmd.chat_id);

                    let mut req =
                        self.bot
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
                        req.reply_parameters =
                            Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
                    }

                    if let Some(thread_id) = cmd.message_thread_id {
                        req.message_thread_id =
                            Some(teloxide::types::ThreadId(MessageId(thread_id)));
                    }

                    if let Err(e) = req.await {
                        self.handle_telegram_error(&subject, e, &prefix).await;
                    }
                }
                Err(e) => error!("Failed to deserialize send location command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle send venue commands
    async fn handle_send_venues(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_send_venue(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self
            .subscriber
            .subscribe::<SendVenueCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
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
                        req.reply_parameters =
                            Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
                    }

                    if let Some(thread_id) = cmd.message_thread_id {
                        req.message_thread_id =
                            Some(teloxide::types::ThreadId(MessageId(thread_id)));
                    }

                    if let Err(e) = req.await {
                        self.handle_telegram_error(&subject, e, &prefix).await;
                    }
                }
                Err(e) => error!("Failed to deserialize send venue command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle send contact commands
    async fn handle_send_contacts(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_send_contact(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self
            .subscriber
            .subscribe::<SendContactCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    debug!("Received send contact command for chat {}", cmd.chat_id);

                    let mut req = self.bot.send_contact(
                        ChatId(cmd.chat_id),
                        cmd.phone_number,
                        cmd.first_name,
                    );

                    if let Some(last_name) = cmd.last_name {
                        req.last_name = Some(last_name);
                    }

                    if let Some(vcard) = cmd.vcard {
                        req.vcard = Some(vcard);
                    }

                    if let Some(reply_to) = cmd.reply_to_message_id {
                        req.reply_parameters =
                            Some(teloxide::types::ReplyParameters::new(MessageId(reply_to)));
                    }

                    if let Some(thread_id) = cmd.message_thread_id {
                        req.message_thread_id =
                            Some(teloxide::types::ThreadId(MessageId(thread_id)));
                    }

                    if let Err(e) = req.await {
                        self.handle_telegram_error(&subject, e, &prefix).await;
                    }
                }
                Err(e) => error!("Failed to deserialize send contact command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle answer callback commands
    async fn handle_answer_callbacks(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::callback_answer(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self
            .subscriber
            .subscribe::<AnswerCallbackCommand>(&subject)
            .await?;

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
                        self.handle_telegram_error(&subject, e, &prefix).await;
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

        let mut stream = self
            .subscriber
            .subscribe::<SendChatActionCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    debug!("Received chat action command for chat {}", cmd.chat_id);

                    let action = convert_chat_action(cmd.action);
                    let mut req = self.bot.send_chat_action(ChatId(cmd.chat_id), action);

                    if let Some(thread_id) = cmd.message_thread_id {
                        req.message_thread_id =
                            Some(teloxide::types::ThreadId(MessageId(thread_id)));
                    }

                    if let Err(e) = req.await {
                        self.handle_telegram_error(&subject, e, &prefix).await;
                    }
                }
                Err(e) => error!("Failed to deserialize chat action command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle forum topic management commands
    async fn handle_forum_topics(&self, prefix: String) -> Result<()> {
        use telegram_nats::subjects;
        use telegram_types::commands::*;

        let create_subject = subjects::agent::forum_create(&prefix);
        let edit_subject = subjects::agent::forum_edit(&prefix);
        let close_subject = subjects::agent::forum_close(&prefix);
        let reopen_subject = subjects::agent::forum_reopen(&prefix);
        let delete_subject = subjects::agent::forum_delete(&prefix);
        let unpin_subject = subjects::agent::forum_unpin(&prefix);
        let edit_general_subject = subjects::agent::forum_edit_general(&prefix);
        let close_general_subject = subjects::agent::forum_close_general(&prefix);
        let reopen_general_subject = subjects::agent::forum_reopen_general(&prefix);
        let hide_general_subject = subjects::agent::forum_hide_general(&prefix);
        let unhide_general_subject = subjects::agent::forum_unhide_general(&prefix);
        let unpin_general_subject = subjects::agent::forum_unpin_general(&prefix);

        info!("Subscribing to forum topic commands");

        let create_stream = self
            .subscriber
            .subscribe::<CreateForumTopicCommand>(&create_subject);
        let edit_stream = self
            .subscriber
            .subscribe::<EditForumTopicCommand>(&edit_subject);
        let close_stream = self
            .subscriber
            .subscribe::<CloseForumTopicCommand>(&close_subject);
        let reopen_stream = self
            .subscriber
            .subscribe::<ReopenForumTopicCommand>(&reopen_subject);
        let delete_stream = self
            .subscriber
            .subscribe::<DeleteForumTopicCommand>(&delete_subject);
        let unpin_stream = self
            .subscriber
            .subscribe::<UnpinAllForumTopicMessagesCommand>(&unpin_subject);
        let edit_general_stream = self
            .subscriber
            .subscribe::<EditGeneralForumTopicCommand>(&edit_general_subject);
        let close_general_stream = self
            .subscriber
            .subscribe::<CloseGeneralForumTopicCommand>(&close_general_subject);
        let reopen_general_stream = self
            .subscriber
            .subscribe::<ReopenGeneralForumTopicCommand>(&reopen_general_subject);
        let hide_general_stream = self
            .subscriber
            .subscribe::<HideGeneralForumTopicCommand>(&hide_general_subject);
        let unhide_general_stream = self
            .subscriber
            .subscribe::<UnhideGeneralForumTopicCommand>(&unhide_general_subject);
        let unpin_general_stream = self
            .subscriber
            .subscribe::<UnpinAllGeneralForumTopicMessagesCommand>(&unpin_general_subject);

        let (
            mut create_stream,
            mut edit_stream,
            mut close_stream,
            mut reopen_stream,
            mut delete_stream,
            mut unpin_stream,
            mut edit_general_stream,
            mut close_general_stream,
            mut reopen_general_stream,
            mut hide_general_stream,
            mut unhide_general_stream,
            mut unpin_general_stream,
        ) = tokio::try_join!(
            create_stream,
            edit_stream,
            close_stream,
            reopen_stream,
            delete_stream,
            unpin_stream,
            edit_general_stream,
            close_general_stream,
            reopen_general_stream,
            hide_general_stream,
            unhide_general_stream,
            unpin_general_stream
        )?;

        loop {
            tokio::select! {
                Some(result) = create_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Creating forum topic in chat {}", cmd.chat_id);
                        // Use default blue color if not specified (0x6FB9F0)
                        let icon_color = cmd.icon_color
                            .map(|c| teloxide::types::Rgb::from_u32(c as u32))
                            .unwrap_or_else(|| teloxide::types::Rgb::from_u32(0x6FB9F0));
                        let icon_custom_emoji_id = cmd.icon_custom_emoji_id.unwrap_or_default();
                        let req = self.bot.create_forum_topic(
                            ChatId(cmd.chat_id),
                            cmd.name,
                            icon_color,
                            icon_custom_emoji_id
                        );
                        if let Err(e) = req.await {
                            self.handle_telegram_error(&create_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = edit_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Editing forum topic {} in chat {}", cmd.message_thread_id, cmd.chat_id);
                        let mut req = self.bot.edit_forum_topic(ChatId(cmd.chat_id), teloxide::types::ThreadId(MessageId(cmd.message_thread_id)));
                        if let Some(name) = cmd.name {
                            req.name = Some(name);
                        }
                        if let Some(emoji_id) = cmd.icon_custom_emoji_id {
                            req.icon_custom_emoji_id = Some(emoji_id);
                        }
                        if let Err(e) = req.await {
                            self.handle_telegram_error(&edit_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = close_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Closing forum topic {} in chat {}", cmd.message_thread_id, cmd.chat_id);
                        if let Err(e) = self.bot.close_forum_topic(ChatId(cmd.chat_id), teloxide::types::ThreadId(MessageId(cmd.message_thread_id))).await {
                            self.handle_telegram_error(&close_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = reopen_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Reopening forum topic {} in chat {}", cmd.message_thread_id, cmd.chat_id);
                        if let Err(e) = self.bot.reopen_forum_topic(ChatId(cmd.chat_id), teloxide::types::ThreadId(MessageId(cmd.message_thread_id))).await {
                            self.handle_telegram_error(&reopen_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = delete_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Deleting forum topic {} in chat {}", cmd.message_thread_id, cmd.chat_id);
                        if let Err(e) = self.bot.delete_forum_topic(ChatId(cmd.chat_id), teloxide::types::ThreadId(MessageId(cmd.message_thread_id))).await {
                            self.handle_telegram_error(&delete_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = unpin_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Unpinning all messages in topic {} in chat {}", cmd.message_thread_id, cmd.chat_id);
                        if let Err(e) = self.bot.unpin_all_forum_topic_messages(ChatId(cmd.chat_id), teloxide::types::ThreadId(MessageId(cmd.message_thread_id))).await {
                            self.handle_telegram_error(&unpin_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = edit_general_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Editing general forum topic in chat {}", cmd.chat_id);
                        if let Err(e) = self.bot.edit_general_forum_topic(ChatId(cmd.chat_id), cmd.name).await {
                            self.handle_telegram_error(&edit_general_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = close_general_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Closing general forum topic in chat {}", cmd.chat_id);
                        if let Err(e) = self.bot.close_general_forum_topic(ChatId(cmd.chat_id)).await {
                            self.handle_telegram_error(&close_general_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = reopen_general_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Reopening general forum topic in chat {}", cmd.chat_id);
                        if let Err(e) = self.bot.reopen_general_forum_topic(ChatId(cmd.chat_id)).await {
                            self.handle_telegram_error(&reopen_general_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = hide_general_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Hiding general forum topic in chat {}", cmd.chat_id);
                        if let Err(e) = self.bot.hide_general_forum_topic(ChatId(cmd.chat_id)).await {
                            self.handle_telegram_error(&hide_general_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = unhide_general_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Unhiding general forum topic in chat {}", cmd.chat_id);
                        if let Err(e) = self.bot.unhide_general_forum_topic(ChatId(cmd.chat_id)).await {
                            self.handle_telegram_error(&unhide_general_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = unpin_general_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Unpinning all general forum topic messages in chat {}", cmd.chat_id);
                        if let Err(e) = self.bot.unpin_all_general_forum_topic_messages(ChatId(cmd.chat_id)).await {
                            self.handle_telegram_error(&unpin_general_subject, e, &prefix).await;
                        }
                    }
                }

                else => break,
            }
        }

        Ok(())
    }

    /// Handle admin commands (permissions, pinning, etc.)
    async fn handle_admin_commands(&self, prefix: String) -> Result<()> {
        use telegram_nats::subjects;
        use telegram_types::commands::*;

        let promote_subject = subjects::agent::admin_promote(&prefix);
        let restrict_subject = subjects::agent::admin_restrict(&prefix);
        let ban_subject = subjects::agent::admin_ban(&prefix);
        let unban_subject = subjects::agent::admin_unban(&prefix);
        let set_permissions_subject = subjects::agent::admin_set_permissions(&prefix);
        let set_title_subject = subjects::agent::admin_set_title(&prefix);
        let pin_subject = subjects::agent::admin_pin(&prefix);
        let unpin_subject = subjects::agent::admin_unpin(&prefix);
        let unpin_all_subject = subjects::agent::admin_unpin_all(&prefix);
        let set_chat_title_subject = subjects::agent::admin_set_chat_title(&prefix);
        let set_chat_description_subject = subjects::agent::admin_set_chat_description(&prefix);

        info!("Subscribing to admin commands");

        let promote_stream = self
            .subscriber
            .subscribe::<PromoteChatMemberCommand>(&promote_subject);
        let restrict_stream = self
            .subscriber
            .subscribe::<RestrictChatMemberCommand>(&restrict_subject);
        let ban_stream = self
            .subscriber
            .subscribe::<BanChatMemberCommand>(&ban_subject);
        let unban_stream = self
            .subscriber
            .subscribe::<UnbanChatMemberCommand>(&unban_subject);
        let set_permissions_stream = self
            .subscriber
            .subscribe::<SetChatPermissionsCommand>(&set_permissions_subject);
        let set_title_stream = self
            .subscriber
            .subscribe::<SetChatAdministratorCustomTitleCommand>(&set_title_subject);
        let pin_stream = self
            .subscriber
            .subscribe::<PinChatMessageCommand>(&pin_subject);
        let unpin_stream = self
            .subscriber
            .subscribe::<UnpinChatMessageCommand>(&unpin_subject);
        let unpin_all_stream = self
            .subscriber
            .subscribe::<UnpinAllChatMessagesCommand>(&unpin_all_subject);
        let set_chat_title_stream = self
            .subscriber
            .subscribe::<SetChatTitleCommand>(&set_chat_title_subject);
        let set_chat_description_stream = self
            .subscriber
            .subscribe::<SetChatDescriptionCommand>(&set_chat_description_subject);

        let (
            mut promote_stream,
            mut restrict_stream,
            mut ban_stream,
            mut unban_stream,
            mut set_permissions_stream,
            mut set_title_stream,
            mut pin_stream,
            mut unpin_stream,
            mut unpin_all_stream,
            mut set_chat_title_stream,
            mut set_chat_description_stream,
        ) = tokio::try_join!(
            promote_stream,
            restrict_stream,
            ban_stream,
            unban_stream,
            set_permissions_stream,
            set_title_stream,
            pin_stream,
            unpin_stream,
            unpin_all_stream,
            set_chat_title_stream,
            set_chat_description_stream
        )?;

        loop {
            tokio::select! {
                Some(result) = promote_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Promoting user {} in chat {}", cmd.user_id, cmd.chat_id);
                        let mut req = self.bot.promote_chat_member(ChatId(cmd.chat_id), teloxide::types::UserId(cmd.user_id as u64));

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
                            self.handle_telegram_error(&promote_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = restrict_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Restricting user {} in chat {}", cmd.user_id, cmd.chat_id);
                        let perms = convert_chat_permissions(&cmd.permissions);
                        let mut req = self.bot.restrict_chat_member(ChatId(cmd.chat_id), teloxide::types::UserId(cmd.user_id as u64), perms);

                        if let Some(until) = cmd.until_date {
                            use chrono::{DateTime, Utc};
                            if let Some(dt) = DateTime::<Utc>::from_timestamp(until, 0) {
                                req.until_date = Some(dt);
                            }
                        }

                        if let Err(e) = req.await {
                            self.handle_telegram_error(&restrict_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = ban_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Banning user {} in chat {}", cmd.user_id, cmd.chat_id);
                        let mut req = self.bot.ban_chat_member(ChatId(cmd.chat_id), teloxide::types::UserId(cmd.user_id as u64));

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
                            self.handle_telegram_error(&ban_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = unban_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Unbanning user {} in chat {}", cmd.user_id, cmd.chat_id);
                        let mut req = self.bot.unban_chat_member(ChatId(cmd.chat_id), teloxide::types::UserId(cmd.user_id as u64));

                        if let Some(only_if_banned) = cmd.only_if_banned {
                            req.only_if_banned = Some(only_if_banned);
                        }

                        if let Err(e) = req.await {
                            self.handle_telegram_error(&unban_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = set_permissions_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Setting permissions in chat {}", cmd.chat_id);
                        let perms = convert_chat_permissions(&cmd.permissions);

                        if let Err(e) = self.bot.set_chat_permissions(ChatId(cmd.chat_id), perms).await {
                            self.handle_telegram_error(&set_permissions_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = set_title_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Setting custom title for user {} in chat {}", cmd.user_id, cmd.chat_id);

                        if let Err(e) = self.bot.set_chat_administrator_custom_title(
                            ChatId(cmd.chat_id),
                            teloxide::types::UserId(cmd.user_id as u64),
                            cmd.custom_title
                        ).await {
                            self.handle_telegram_error(&set_title_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = pin_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Pinning message {} in chat {}", cmd.message_id, cmd.chat_id);
                        let mut req = self.bot.pin_chat_message(ChatId(cmd.chat_id), MessageId(cmd.message_id));

                        if let Some(disable_notification) = cmd.disable_notification {
                            req.disable_notification = Some(disable_notification);
                        }

                        if let Err(e) = req.await {
                            self.handle_telegram_error(&pin_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = unpin_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Unpinning message in chat {}", cmd.chat_id);
                        let mut req = self.bot.unpin_chat_message(ChatId(cmd.chat_id));

                        if let Some(message_id) = cmd.message_id {
                            req.message_id = Some(MessageId(message_id));
                        }

                        if let Err(e) = req.await {
                            self.handle_telegram_error(&unpin_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = unpin_all_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Unpinning all messages in chat {}", cmd.chat_id);

                        if let Err(e) = self.bot.unpin_all_chat_messages(ChatId(cmd.chat_id)).await {
                            self.handle_telegram_error(&unpin_all_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = set_chat_title_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Setting chat title in chat {}", cmd.chat_id);

                        if let Err(e) = self.bot.set_chat_title(ChatId(cmd.chat_id), cmd.title).await {
                            self.handle_telegram_error(&set_chat_title_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = set_chat_description_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Setting chat description in chat {}", cmd.chat_id);

                        let mut req = self.bot.set_chat_description(ChatId(cmd.chat_id));
                        req.description = Some(cmd.description);

                        if let Err(e) = req.await {
                            self.handle_telegram_error(&set_chat_description_subject, e, &prefix).await;
                        }
                    }
                }

                else => break,
            }
        }

        Ok(())
    }

    /// Handle file download commands (getFile and download)
    async fn handle_file_commands(&self, prefix: String) -> Result<()> {
        use telegram_nats::subjects;
        use telegram_types::commands::*;

        let get_file_subject = subjects::agent::file_get(&prefix);
        let download_file_subject = subjects::agent::file_download(&prefix);

        info!("Subscribing to file commands");

        let get_file_stream = self
            .subscriber
            .subscribe::<GetFileCommand>(&get_file_subject);
        let download_file_stream = self
            .subscriber
            .subscribe::<DownloadFileCommand>(&download_file_subject);

        let (mut get_file_stream, mut download_file_stream) =
            tokio::try_join!(get_file_stream, download_file_stream)?;

        loop {
            tokio::select! {
                Some(result) = get_file_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Getting file info for file_id: {}", cmd.file_id);

                        match self.bot.get_file(&cmd.file_id).await {
                            Ok(file) => {
                                // Construct download URL
                                let bot_token = self.bot.token();
                                let download_url = format!(
                                    "https://api.telegram.org/file/bot{}/{}",
                                    bot_token,
                                    file.path
                                );

                                let response = FileInfoResponse {
                                    file_id: cmd.file_id.clone(),
                                    file_unique_id: file.unique_id.clone(),
                                    file_size: Some(file.size as u64),
                                    file_path: file.path.clone(),
                                    download_url,
                                    request_id: cmd.request_id,
                                };

                                // Publish response
                                let response_subject = subjects::bot::file_info(&prefix);
                                if let Err(e) = self.subscriber.client()
                                    .publish(response_subject.clone(), serde_json::to_vec(&response).unwrap().into())
                                    .await
                                {
                                    error!("Failed to publish file info response: {}", e);
                                } else {
                                    debug!("Published file info response to {}", response_subject);
                                }
                            }
                            Err(e) => {
                                self.handle_telegram_error(&get_file_subject, e, &prefix).await;
                            }
                        }
                    }
                }

                Some(result) = download_file_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Downloading file {} to {}", cmd.file_id, cmd.destination_path);

                        // Get file info first
                        match self.bot.get_file(&cmd.file_id).await {
                            Ok(file) => {
                                // Construct download URL
                                let bot_token = self.bot.token();
                                let download_url = format!(
                                    "https://api.telegram.org/file/bot{}/{}",
                                    bot_token,
                                    file.path
                                );

                                // Download file using reqwest
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

                                        // Publish response
                                        let response_subject = subjects::bot::file_downloaded(&prefix);
                                        if let Err(e) = self.subscriber.client()
                                            .publish(response_subject.clone(), serde_json::to_vec(&response).unwrap().into())
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

                                        // Publish error response
                                        let response_subject = subjects::bot::file_downloaded(&prefix);
                                        if let Err(e) = self.subscriber.client()
                                            .publish(response_subject.clone(), serde_json::to_vec(&response).unwrap().into())
                                            .await
                                        {
                                            error!("Failed to publish file download error response: {}", e);
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                let e_str = e.to_string();
                                self.handle_telegram_error(&download_file_subject, e, &prefix).await;
                                let response = FileDownloadResponse {
                                    file_id: cmd.file_id.clone(),
                                    local_path: cmd.destination_path.clone(),
                                    file_size: 0,
                                    success: false,
                                    error: Some(format!("Failed to get file info: {}", e_str)),
                                    request_id: cmd.request_id,
                                };

                                // Publish error response
                                let response_subject = subjects::bot::file_downloaded(&prefix);
                                if let Err(e) = self.subscriber.client()
                                    .publish(response_subject.clone(), serde_json::to_vec(&response).unwrap().into())
                                    .await
                                {
                                    error!("Failed to publish file download error response: {}", e);
                                }
                            }
                        }
                    }
                }

                else => break,
            }
        }

        Ok(())
    }

    /// Handle payment commands (invoices, pre-checkout, shipping)
    async fn handle_payment_commands(&self, prefix: String) -> Result<()> {
        use telegram_nats::subjects;
        use telegram_types::commands::*;

        let send_invoice_subject = subjects::agent::payment_send_invoice(&prefix);
        let answer_pre_checkout_subject = subjects::agent::payment_answer_pre_checkout(&prefix);
        let answer_shipping_subject = subjects::agent::payment_answer_shipping(&prefix);

        info!("Subscribing to payment commands");

        let send_invoice_stream = self
            .subscriber
            .subscribe::<SendInvoiceCommand>(&send_invoice_subject);
        let answer_pre_checkout_stream = self
            .subscriber
            .subscribe::<AnswerPreCheckoutQueryCommand>(&answer_pre_checkout_subject);
        let answer_shipping_stream = self
            .subscriber
            .subscribe::<AnswerShippingQueryCommand>(&answer_shipping_subject);

        let (mut send_invoice_stream, mut answer_pre_checkout_stream, mut answer_shipping_stream) =
            tokio::try_join!(
                send_invoice_stream,
                answer_pre_checkout_stream,
                answer_shipping_stream
            )?;

        loop {
            tokio::select! {
                Some(result) = send_invoice_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Sending invoice to chat {}", cmd.chat_id);

                        // Convert prices (amount in smallest currency units as u32)
                        let prices: Vec<teloxide::types::LabeledPrice> = cmd.prices.iter()
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
                            prices
                        );

                        // Set optional fields
                        if !cmd.provider_token.is_empty() {
                            req.provider_token = Some(cmd.provider_token);
                        }
                        req.max_tip_amount = cmd.max_tip_amount.map(|a| a as u32);
                        req.suggested_tip_amounts = cmd.suggested_tip_amounts.map(|amounts|
                            amounts.iter().map(|&a| a as u32).collect()
                        );
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
                            self.handle_telegram_error(&send_invoice_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = answer_pre_checkout_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Answering pre-checkout query {}: ok={}", cmd.pre_checkout_query_id, cmd.ok);

                        let req = if cmd.ok {
                            self.bot.answer_pre_checkout_query(&cmd.pre_checkout_query_id, true)
                        } else {
                            let mut req = self.bot.answer_pre_checkout_query(&cmd.pre_checkout_query_id, false);
                            req.error_message = cmd.error_message;
                            req
                        };

                        if let Err(e) = req.await {
                            self.handle_telegram_error(&answer_pre_checkout_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = answer_shipping_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Answering shipping query {}: ok={}", cmd.shipping_query_id, cmd.ok);

                        let mut req = self.bot.answer_shipping_query(&cmd.shipping_query_id, cmd.ok);

                        if cmd.ok {
                            let options: Vec<teloxide::types::ShippingOption> = cmd.shipping_options.unwrap_or_default()
                                .into_iter()
                                .map(|opt| teloxide::types::ShippingOption {
                                    id: opt.id,
                                    title: opt.title,
                                    prices: opt.prices.iter().map(|p| teloxide::types::LabeledPrice {
                                        label: p.label.clone(),
                                        amount: p.amount as u32,
                                    }).collect(),
                                })
                                .collect();
                            req.shipping_options = Some(options);
                        } else {
                            req.error_message = cmd.error_message;
                        }

                        if let Err(e) = req.await {
                            self.handle_telegram_error(&answer_shipping_subject, e, &prefix).await;
                        }
                    }
                }

                else => break,
            }
        }

        Ok(())
    }

    /// Handle bot commands setup (setMyCommands, deleteMyCommands, getMyCommands)
    async fn handle_bot_commands(&self, prefix: String) -> Result<()> {
        use telegram_nats::subjects;
        use telegram_types::commands::*;

        let set_commands_subject = subjects::agent::bot_commands_set(&prefix);
        let delete_commands_subject = subjects::agent::bot_commands_delete(&prefix);
        let get_commands_subject = subjects::agent::bot_commands_get(&prefix);

        info!("Subscribing to bot commands");

        let set_commands_stream = self
            .subscriber
            .subscribe::<SetMyCommandsCommand>(&set_commands_subject);
        let delete_commands_stream = self
            .subscriber
            .subscribe::<DeleteMyCommandsCommand>(&delete_commands_subject);
        let get_commands_stream = self
            .subscriber
            .subscribe::<GetMyCommandsCommand>(&get_commands_subject);

        let (mut set_commands_stream, mut delete_commands_stream, mut get_commands_stream) = tokio::try_join!(
            set_commands_stream,
            delete_commands_stream,
            get_commands_stream
        )?;

        loop {
            tokio::select! {
                Some(result) = set_commands_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Setting bot commands ({} commands)", cmd.commands.len());

                        let commands: Vec<teloxide::types::BotCommand> = cmd.commands.iter()
                            .map(|c| teloxide::types::BotCommand::new(c.command.clone(), c.description.clone()))
                            .collect();

                        let mut req = self.bot.set_my_commands(commands);

                        if let Some(scope) = cmd.scope {
                            req.scope = Some(convert_bot_command_scope(scope));
                        }

                        req.language_code = cmd.language_code;

                        if let Err(e) = req.await {
                            self.handle_telegram_error(&set_commands_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = delete_commands_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Deleting bot commands");

                        let mut req = self.bot.delete_my_commands();

                        if let Some(scope) = cmd.scope {
                            req.scope = Some(convert_bot_command_scope(scope));
                        }

                        req.language_code = cmd.language_code;

                        if let Err(e) = req.await {
                            self.handle_telegram_error(&delete_commands_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = get_commands_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Getting bot commands");

                        let mut req = self.bot.get_my_commands();

                        if let Some(scope) = cmd.scope {
                            req.scope = Some(convert_bot_command_scope(scope));
                        }

                        req.language_code = cmd.language_code;

                        match req.await {
                            Ok(commands) => {
                                let response = BotCommandsResponse {
                                    commands: commands.iter().map(|c| telegram_types::chat::BotCommand {
                                        command: c.command.clone(),
                                        description: c.description.clone(),
                                    }).collect(),
                                    request_id: cmd.request_id,
                                };

                                // Publish response
                                let response_subject = subjects::bot::bot_commands_response(&prefix);
                                if let Err(e) = self.subscriber.client()
                                    .publish(response_subject.clone(), serde_json::to_vec(&response).unwrap().into())
                                    .await
                                {
                                    error!("Failed to publish bot commands response: {}", e);
                                } else {
                                    debug!("Published bot commands response to {}", response_subject);
                                }
                            }
                            Err(e) => {
                                self.handle_telegram_error(&get_commands_subject, e, &prefix).await;
                            }
                        }
                    }
                }

                else => break,
            }
        }

        Ok(())
    }

    /// Handle sticker set management commands
    async fn handle_sticker_management(&self, prefix: String) -> Result<()> {
        use telegram_types::commands::*;

        let get_set_subject = subjects::agent::sticker_get_set(&prefix);
        let upload_subject = subjects::agent::sticker_upload_file(&prefix);
        let create_subject = subjects::agent::sticker_create_set(&prefix);
        let add_subject = subjects::agent::sticker_add_to_set(&prefix);
        let set_position_subject = subjects::agent::sticker_set_position(&prefix);
        let delete_from_subject = subjects::agent::sticker_delete_from_set(&prefix);
        let set_title_subject = subjects::agent::sticker_set_title(&prefix);
        let set_thumbnail_subject = subjects::agent::sticker_set_thumbnail(&prefix);
        let delete_set_subject = subjects::agent::sticker_delete_set(&prefix);
        let set_emoji_subject = subjects::agent::sticker_set_emoji_list(&prefix);
        let set_keywords_subject = subjects::agent::sticker_set_keywords(&prefix);
        let set_mask_subject = subjects::agent::sticker_set_mask_position(&prefix);

        info!("Subscribing to sticker management commands");

        let (
            mut get_set_stream,
            mut upload_stream,
            mut create_stream,
            mut add_stream,
            mut set_position_stream,
            mut delete_from_stream,
            mut set_title_stream,
            mut set_thumbnail_stream,
            mut delete_set_stream,
            mut set_emoji_stream,
            mut set_keywords_stream,
            mut set_mask_stream,
        ) = tokio::try_join!(
            self.subscriber
                .subscribe::<GetStickerSetCommand>(&get_set_subject),
            self.subscriber
                .subscribe::<UploadStickerFileCommand>(&upload_subject),
            self.subscriber
                .subscribe::<CreateNewStickerSetCommand>(&create_subject),
            self.subscriber
                .subscribe::<AddStickerToSetCommand>(&add_subject),
            self.subscriber
                .subscribe::<SetStickerPositionInSetCommand>(&set_position_subject),
            self.subscriber
                .subscribe::<DeleteStickerFromSetCommand>(&delete_from_subject),
            self.subscriber
                .subscribe::<SetStickerSetTitleCommand>(&set_title_subject),
            self.subscriber
                .subscribe::<SetStickerSetThumbnailCommand>(&set_thumbnail_subject),
            self.subscriber
                .subscribe::<DeleteStickerSetCommand>(&delete_set_subject),
            self.subscriber
                .subscribe::<SetStickerEmojiListCommand>(&set_emoji_subject),
            self.subscriber
                .subscribe::<SetStickerKeywordsCommand>(&set_keywords_subject),
            self.subscriber
                .subscribe::<SetStickerMaskPositionCommand>(&set_mask_subject),
        )?;

        loop {
            tokio::select! {
                Some(result) = get_set_stream.next() => {
                    if let Ok(cmd) = result {
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
                                let subj = subjects::bot::sticker_set_info(&prefix);
                                if let Ok(payload) = serde_json::to_vec(&response) {
                                    let _ = self.subscriber.client().publish(subj, payload.into()).await;
                                }
                            }
                            Err(e) => self.handle_telegram_error(&get_set_subject, e, &prefix).await,
                        }
                    }
                }

                Some(result) = upload_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Uploading sticker file for user {}", cmd.user_id);
                        let sticker_file = teloxide::types::InputFile::file_id(&cmd.sticker);
                        let format = convert_sticker_format(&cmd.format);
                        match self.bot.upload_sticker_file(teloxide::types::UserId(cmd.user_id as u64), sticker_file, format).await {
                            Ok(file) => {
                                let response = UploadStickerFileResponse {
                                    file_id: file.id.clone(),
                                    request_id: cmd.request_id,
                                };
                                let subj = subjects::bot::sticker_uploaded(&prefix);
                                if let Ok(payload) = serde_json::to_vec(&response) {
                                    let _ = self.subscriber.client().publish(subj, payload.into()).await;
                                }
                            }
                            Err(e) => self.handle_telegram_error(&upload_subject, e, &prefix).await,
                        }
                    }
                }

                Some(result) = create_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Creating sticker set '{}' for user {}", cmd.name, cmd.user_id);
                        let stickers: Vec<teloxide::types::InputSticker> = cmd.stickers.iter()
                            .map(|s| convert_input_sticker(s))
                            .collect();
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
                            self.handle_telegram_error(&create_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = add_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Adding sticker to set '{}' for user {}", cmd.name, cmd.user_id);
                        let sticker = convert_input_sticker(&cmd.sticker);
                        if let Err(e) = self.bot.add_sticker_to_set(
                            teloxide::types::UserId(cmd.user_id as u64),
                            &cmd.name,
                            sticker,
                        ).await {
                            self.handle_telegram_error(&add_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = set_position_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Setting sticker position to {}", cmd.position);
                        if let Err(e) = self.bot.set_sticker_position_in_set(&cmd.sticker, cmd.position).await {
                            self.handle_telegram_error(&set_position_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = delete_from_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Deleting sticker from set: {}", cmd.sticker);
                        if let Err(e) = self.bot.delete_sticker_from_set(&cmd.sticker).await {
                            self.handle_telegram_error(&delete_from_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = set_title_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Setting title of sticker set '{}' to '{}'", cmd.name, cmd.title);
                        if let Err(e) = self.bot.set_sticker_set_title(&cmd.name, &cmd.title).await {
                            self.handle_telegram_error(&set_title_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = set_thumbnail_stream.next() => {
                    if let Ok(cmd) = result {
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
                            self.handle_telegram_error(&set_thumbnail_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = delete_set_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Deleting sticker set '{}'", cmd.name);
                        if let Err(e) = self.bot.delete_sticker_set(&cmd.name).await {
                            self.handle_telegram_error(&delete_set_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = set_emoji_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Setting emoji list for sticker {}", cmd.sticker);
                        if let Err(e) = self.bot.set_sticker_emoji_list(&cmd.sticker, cmd.emoji_list).await {
                            self.handle_telegram_error(&set_emoji_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = set_keywords_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Setting keywords for sticker {}", cmd.sticker);
                        let mut req = self.bot.set_sticker_keywords(&cmd.sticker);
                        req.keywords = Some(cmd.keywords);
                        if let Err(e) = req.await {
                            self.handle_telegram_error(&set_keywords_subject, e, &prefix).await;
                        }
                    }
                }

                Some(result) = set_mask_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Setting mask position for sticker {}", cmd.sticker);
                        let mut req = self.bot.set_sticker_mask_position(&cmd.sticker);
                        if let Some(mp) = cmd.mask_position {
                            req.mask_position = Some(convert_mask_position(mp));
                        }
                        if let Err(e) = req.await {
                            self.handle_telegram_error(&set_mask_subject, e, &prefix).await;
                        }
                    }
                }

                else => break,
            }
        }

        Ok(())
    }

    /// Handle forwardMessage commands
    async fn handle_forward_messages(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_forward(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self
            .subscriber
            .subscribe::<ForwardMessageCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
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
                        req.message_thread_id =
                            Some(teloxide::types::ThreadId(MessageId(thread_id)));
                    }
                    req.disable_notification = cmd.disable_notification;
                    req.protect_content = cmd.protect_content;

                    if let Err(e) = req.await {
                        self.handle_telegram_error(&subject, e, &prefix).await;
                    }
                }
                Err(e) => error!("Failed to deserialize forward message command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle copyMessage commands
    async fn handle_copy_messages(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_copy(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self
            .subscriber
            .subscribe::<CopyMessageCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
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
                        req.message_thread_id =
                            Some(teloxide::types::ThreadId(MessageId(thread_id)));
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
                        req.reply_parameters =
                            Some(teloxide::types::ReplyParameters::new(MessageId(reply_id)));
                    }
                    req.disable_notification = cmd.disable_notification;
                    req.protect_content = cmd.protect_content;

                    if let Err(e) = req.await {
                        self.handle_telegram_error(&subject, e, &prefix).await;
                    }
                }
                Err(e) => error!("Failed to deserialize copy message command: {}", e),
            }
        }

        Ok(())
    }

    /// Classify a Telegram API error and publish a CommandErrorEvent to NATS.
    ///
    /// - `Retry`    → logs a warning (actual re-execution is the caller's responsibility)
    /// - `Migrated` → logs the new chat ID
    /// - `Permanent` / `Transient` → publishes the event to `telegram.{prefix}.bot.error.command`
    pub(crate) async fn handle_telegram_error(
        &self,
        command_subject: &str,
        err: teloxide::RequestError,
        prefix: &str,
    ) {
        let evt = match classify(command_subject, &err) {
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
                }
            }
            ErrorOutcome::Migrated(new_chat_id) => {
                warn!(
                    "Chat migrated on '{}': new chat_id={}",
                    command_subject, new_chat_id
                );
                telegram_types::errors::CommandErrorEvent {
                    command_subject: command_subject.to_string(),
                    error_code: telegram_types::errors::TelegramErrorCode::MigrateToChatId,
                    category: telegram_types::errors::ErrorCategory::ChatMigrated,
                    message: format!("Chat migrated to new ID: {}", new_chat_id.0),
                    retry_after_secs: None,
                    migrated_to_chat_id: Some(new_chat_id.0),
                    is_permanent: false,
                }
            }
            ErrorOutcome::Permanent(evt) | ErrorOutcome::Transient(evt) => evt,
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
        ChatAction::ChooseSticker => teloxide::types::ChatAction::Typing, // no direct equivalent in this teloxide version
        ChatAction::FindLocation => teloxide::types::ChatAction::FindLocation,
    }
}

impl OutboundProcessor {
    /// Handle answer inline query commands
    async fn handle_answer_inline_queries(&self, prefix: String) -> Result<()> {
        use telegram_types::commands::AnswerInlineQueryCommand;

        let subject = subjects::agent::inline_answer(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self
            .subscriber
            .subscribe::<AnswerInlineQueryCommand>(&subject)
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    debug!(
                        "Received answer inline query command for query {}",
                        cmd.inline_query_id
                    );

                    // Convert our inline results to teloxide format
                    let results: Vec<teloxide::types::InlineQueryResult> = cmd
                        .results
                        .into_iter()
                        .filter_map(|r| convert_inline_query_result(r))
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
                        self.handle_telegram_error(&subject, e, &prefix).await;
                    }
                }
                Err(e) => error!("Failed to deserialize answer inline query command: {}", e),
            }
        }

        Ok(())
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
