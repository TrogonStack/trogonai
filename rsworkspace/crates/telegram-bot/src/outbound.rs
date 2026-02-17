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
        let inline_task = self.handle_answer_inline_queries(prefix.clone());
        let forum_task = self.handle_forum_topics(prefix.clone());

        // Run all tasks concurrently
        tokio::try_join!(
            send_task,
            edit_task,
            delete_task,
            photo_task,
            callback_task,
            action_task,
            stream_task,
            inline_task,
            forum_task
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

                    if let Some(thread_id) = cmd.message_thread_id {
                        req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
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

                    if let Some(thread_id) = cmd.message_thread_id {
                        req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
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
                    let mut req = self.bot.send_chat_action(ChatId(cmd.chat_id), action);

                    if let Some(thread_id) = cmd.message_thread_id {
                        req.message_thread_id = Some(teloxide::types::ThreadId(MessageId(thread_id)));
                    }

                    if let Err(e) = req.await {
                        warn!("Failed to send chat action: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize chat action command: {}", e),
            }
        }

        Ok(())
    }

    /// Handle forum topic management commands
    async fn handle_forum_topics(&self, prefix: String) -> Result<()> {
        use telegram_types::commands::*;
        use telegram_nats::subjects;

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

        let create_stream = self.subscriber.subscribe::<CreateForumTopicCommand>(&create_subject);
        let edit_stream = self.subscriber.subscribe::<EditForumTopicCommand>(&edit_subject);
        let close_stream = self.subscriber.subscribe::<CloseForumTopicCommand>(&close_subject);
        let reopen_stream = self.subscriber.subscribe::<ReopenForumTopicCommand>(&reopen_subject);
        let delete_stream = self.subscriber.subscribe::<DeleteForumTopicCommand>(&delete_subject);
        let unpin_stream = self.subscriber.subscribe::<UnpinAllForumTopicMessagesCommand>(&unpin_subject);
        let edit_general_stream = self.subscriber.subscribe::<EditGeneralForumTopicCommand>(&edit_general_subject);
        let close_general_stream = self.subscriber.subscribe::<CloseGeneralForumTopicCommand>(&close_general_subject);
        let reopen_general_stream = self.subscriber.subscribe::<ReopenGeneralForumTopicCommand>(&reopen_general_subject);
        let hide_general_stream = self.subscriber.subscribe::<HideGeneralForumTopicCommand>(&hide_general_subject);
        let unhide_general_stream = self.subscriber.subscribe::<UnhideGeneralForumTopicCommand>(&unhide_general_subject);
        let unpin_general_stream = self.subscriber.subscribe::<UnpinAllGeneralForumTopicMessagesCommand>(&unpin_general_subject);

        let (mut create_stream, mut edit_stream, mut close_stream, mut reopen_stream,
             mut delete_stream, mut unpin_stream, mut edit_general_stream, mut close_general_stream,
             mut reopen_general_stream, mut hide_general_stream, mut unhide_general_stream,
             mut unpin_general_stream) = tokio::try_join!(
            create_stream, edit_stream, close_stream, reopen_stream,
            delete_stream, unpin_stream, edit_general_stream, close_general_stream,
            reopen_general_stream, hide_general_stream, unhide_general_stream,
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
                            error!("Failed to create forum topic: {}", e);
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
                            error!("Failed to edit forum topic: {}", e);
                        }
                    }
                }

                Some(result) = close_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Closing forum topic {} in chat {}", cmd.message_thread_id, cmd.chat_id);
                        if let Err(e) = self.bot.close_forum_topic(ChatId(cmd.chat_id), teloxide::types::ThreadId(MessageId(cmd.message_thread_id))).await {
                            error!("Failed to close forum topic: {}", e);
                        }
                    }
                }

                Some(result) = reopen_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Reopening forum topic {} in chat {}", cmd.message_thread_id, cmd.chat_id);
                        if let Err(e) = self.bot.reopen_forum_topic(ChatId(cmd.chat_id), teloxide::types::ThreadId(MessageId(cmd.message_thread_id))).await {
                            error!("Failed to reopen forum topic: {}", e);
                        }
                    }
                }

                Some(result) = delete_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Deleting forum topic {} in chat {}", cmd.message_thread_id, cmd.chat_id);
                        if let Err(e) = self.bot.delete_forum_topic(ChatId(cmd.chat_id), teloxide::types::ThreadId(MessageId(cmd.message_thread_id))).await {
                            error!("Failed to delete forum topic: {}", e);
                        }
                    }
                }

                Some(result) = unpin_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Unpinning all messages in topic {} in chat {}", cmd.message_thread_id, cmd.chat_id);
                        if let Err(e) = self.bot.unpin_all_forum_topic_messages(ChatId(cmd.chat_id), teloxide::types::ThreadId(MessageId(cmd.message_thread_id))).await {
                            error!("Failed to unpin forum topic messages: {}", e);
                        }
                    }
                }

                Some(result) = edit_general_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Editing general forum topic in chat {}", cmd.chat_id);
                        if let Err(e) = self.bot.edit_general_forum_topic(ChatId(cmd.chat_id), cmd.name).await {
                            error!("Failed to edit general forum topic: {}", e);
                        }
                    }
                }

                Some(result) = close_general_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Closing general forum topic in chat {}", cmd.chat_id);
                        if let Err(e) = self.bot.close_general_forum_topic(ChatId(cmd.chat_id)).await {
                            error!("Failed to close general forum topic: {}", e);
                        }
                    }
                }

                Some(result) = reopen_general_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Reopening general forum topic in chat {}", cmd.chat_id);
                        if let Err(e) = self.bot.reopen_general_forum_topic(ChatId(cmd.chat_id)).await {
                            error!("Failed to reopen general forum topic: {}", e);
                        }
                    }
                }

                Some(result) = hide_general_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Hiding general forum topic in chat {}", cmd.chat_id);
                        if let Err(e) = self.bot.hide_general_forum_topic(ChatId(cmd.chat_id)).await {
                            error!("Failed to hide general forum topic: {}", e);
                        }
                    }
                }

                Some(result) = unhide_general_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Unhiding general forum topic in chat {}", cmd.chat_id);
                        if let Err(e) = self.bot.unhide_general_forum_topic(ChatId(cmd.chat_id)).await {
                            error!("Failed to unhide general forum topic: {}", e);
                        }
                    }
                }

                Some(result) = unpin_general_stream.next() => {
                    if let Ok(cmd) = result {
                        debug!("Unpinning all general forum topic messages in chat {}", cmd.chat_id);
                        if let Err(e) = self.bot.unpin_all_general_forum_topic_messages(ChatId(cmd.chat_id)).await {
                            error!("Failed to unpin general forum topic messages: {}", e);
                        }
                    }
                }

                else => break,
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

impl OutboundProcessor {
    /// Handle answer inline query commands
    async fn handle_answer_inline_queries(&self, prefix: String) -> Result<()> {
        use telegram_types::commands::AnswerInlineQueryCommand;

        let subject = subjects::agent::inline_answer(&prefix);
        info!("Subscribing to {}", subject);

        let mut stream = self.subscriber.subscribe::<AnswerInlineQueryCommand>(&subject).await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    debug!("Received answer inline query command for query {}", cmd.inline_query_id);

                    // Convert our inline results to teloxide format
                    let results: Vec<teloxide::types::InlineQueryResult> = cmd.results
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
                        error!("Failed to answer inline query: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize answer inline query command: {}", e),
            }
        }

        Ok(())
    }
}

/// Convert our InlineQueryResult to Teloxide's
fn convert_inline_query_result(result: telegram_types::commands::InlineQueryResult) -> Option<teloxide::types::InlineQueryResult> {
    use teloxide::types::{
        InlineQueryResult as TgResult, InlineQueryResultArticle, InlineQueryResultPhoto,
        InlineQueryResultCachedPhoto, InlineQueryResultCachedSticker, InlineQueryResultGif,
        InlineQueryResultCachedGif, InlineQueryResultMpeg4Gif, InlineQueryResultCachedMpeg4Gif,
        InlineQueryResultVideo, InlineQueryResultCachedVideo, InlineQueryResultAudio,
        InlineQueryResultCachedAudio, InlineQueryResultVoice, InlineQueryResultCachedVoice,
        InlineQueryResultDocument, InlineQueryResultCachedDocument, InlineQueryResultLocation,
        InlineQueryResultVenue, InlineQueryResultContact, InlineQueryResultGame,
        InputMessageContent, InputMessageContentText
    };
    use telegram_types::commands::InlineQueryResult as OurResult;
    use url::Url;

    match result {
        OurResult::Article(article) => {
            let input_content = InputMessageContent::Text(InputMessageContentText::new(
                article.input_message_content.message_text
            ));

            let mut result = InlineQueryResultArticle::new(
                article.id,
                article.title,
                input_content
            );

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
            let mut result = InlineQueryResultCachedSticker::new(
                sticker.id,
                sticker.sticker_file_id
            );

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
            let mut result = InlineQueryResultCachedGif::new(
                gif.id,
                gif.gif_file_id
            );

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
            result.mpeg4_duration = mp4.mpeg4_duration.map(teloxide::types::Seconds::from_seconds);
            result.title = mp4.title;
            result.caption = mp4.caption;
            result.parse_mode = mp4.parse_mode.map(convert_parse_mode);

            Some(TgResult::Mpeg4Gif(result))
        }

        OurResult::CachedMpeg4Gif(mp4) => {
            let mut result = InlineQueryResultCachedMpeg4Gif::new(
                mp4.id,
                mp4.mpeg4_file_id
            );

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

            let mut result = InlineQueryResultVideo::new(
                video.id,
                video_url,
                mime_type,
                thumb_url,
                video.title
            );

            result.caption = video.caption;
            result.parse_mode = video.parse_mode.map(convert_parse_mode);
            result.video_width = video.video_width;
            result.video_height = video.video_height;
            result.video_duration = video.video_duration.map(teloxide::types::Seconds::from_seconds);
            result.description = video.description;

            Some(TgResult::Video(result))
        }

        OurResult::CachedVideo(video) => {
            let mut result = InlineQueryResultCachedVideo::new(
                video.id,
                video.video_file_id,
                video.title
            );

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

            let mut result = InlineQueryResultAudio::new(
                audio.id,
                audio_url,
                audio.title
            );

            result.caption = audio.caption;
            result.parse_mode = audio.parse_mode.map(convert_parse_mode);
            result.performer = audio.performer;
            result.audio_duration = audio.audio_duration.map(teloxide::types::Seconds::from_seconds);

            Some(TgResult::Audio(result))
        }

        OurResult::CachedAudio(audio) => {
            let mut result = InlineQueryResultCachedAudio::new(
                audio.id,
                audio.audio_file_id
            );

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

            let mut result = InlineQueryResultVoice::new(
                voice.id,
                voice_url,
                voice.title
            );

            result.caption = voice.caption;
            result.parse_mode = voice.parse_mode.map(convert_parse_mode);
            result.voice_duration = voice.voice_duration.map(teloxide::types::Seconds::from_seconds);

            Some(TgResult::Voice(result))
        }

        OurResult::CachedVoice(voice) => {
            let mut result = InlineQueryResultCachedVoice::new(
                voice.id,
                voice.voice_file_id,
                voice.title
            );

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

            let thumbnail_url = document.thumbnail_url
                .and_then(|s| Url::parse(&s).ok());

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
                document.document_file_id
            );

            result.description = document.description;
            result.caption = document.caption;
            result.parse_mode = document.parse_mode.map(convert_parse_mode);

            Some(TgResult::CachedDocument(result))
        }

        OurResult::CachedPhoto(photo) => {
            let mut result = InlineQueryResultCachedPhoto::new(
                photo.id,
                photo.photo_file_id
            );

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
                location.longitude
            );

            result.horizontal_accuracy = location.horizontal_accuracy;
            result.live_period = location.live_period.map(|s| teloxide::types::LivePeriod::from_seconds(teloxide::types::Seconds::from_seconds(s)));
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
                venue.address
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
            let mut result = InlineQueryResultContact::new(
                contact.id,
                contact.phone_number,
                contact.first_name
            );

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
            let result = InlineQueryResultGame::new(
                game.id,
                game.game_short_name
            );

            Some(TgResult::Game(result))
        }
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
