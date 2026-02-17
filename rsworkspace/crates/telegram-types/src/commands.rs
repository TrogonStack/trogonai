//! Commands sent from agents to Telegram bot via NATS

use serde::{Deserialize, Serialize};

use crate::chat::InlineKeyboardMarkup;

/// Parse mode for message formatting
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ParseMode {
    Markdown,
    MarkdownV2,
    HTML,
}

/// Chat action types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatAction {
    Typing,
    UploadPhoto,
    RecordVideo,
    UploadVideo,
    RecordVoice,
    UploadVoice,
    UploadDocument,
    ChooseSticker,
    FindLocation,
}

/// Send a text message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageCommand {
    pub chat_id: i64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
    /// Unique identifier for the target message thread (topic) of the forum; for forum supergroups only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
}

/// Edit an existing message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditMessageCommand {
    pub chat_id: i64,
    pub message_id: i32,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
}

/// Delete a message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteMessageCommand {
    pub chat_id: i64,
    pub message_id: i32,
}

/// Send a photo
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendPhotoCommand {
    pub chat_id: i64,
    pub photo: String, // file_id or URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i32>,
    /// Unique identifier for the target message thread (topic) of the forum; for forum supergroups only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
}

/// Answer a callback query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerCallbackCommand {
    pub callback_query_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_alert: Option<bool>,
}

/// Send chat action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendChatActionCommand {
    pub chat_id: i64,
    pub action: ChatAction,
    /// Unique identifier for the target message thread (topic) of the forum; for forum supergroups only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
}

/// Stream partial message updates (for progressive display)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamMessageCommand {
    pub chat_id: i64,
    /// Message ID if editing, None for new message
    pub message_id: Option<i32>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
    /// Whether this is the final chunk
    pub is_final: bool,
    /// Session ID for tracking (optional, defaults to chat-based ID)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Unique identifier for the target message thread (topic) of the forum; for forum supergroups only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
}

/// Answer inline query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerInlineQueryCommand {
    pub inline_query_id: String,
    pub results: Vec<InlineQueryResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_time: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_personal: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<String>,
}

/// Inline query result types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InlineQueryResult {
    Article(InlineQueryResultArticle),
    Photo(InlineQueryResultPhoto),
    CachedPhoto(InlineQueryResultCachedPhoto),
    CachedSticker(InlineQueryResultCachedSticker),
    Gif(InlineQueryResultGif),
    CachedGif(InlineQueryResultCachedGif),
    Mpeg4Gif(InlineQueryResultMpeg4Gif),
    CachedMpeg4Gif(InlineQueryResultCachedMpeg4Gif),
    Video(InlineQueryResultVideo),
    CachedVideo(InlineQueryResultCachedVideo),
    Audio(InlineQueryResultAudio),
    CachedAudio(InlineQueryResultCachedAudio),
    Voice(InlineQueryResultVoice),
    CachedVoice(InlineQueryResultCachedVoice),
    Document(InlineQueryResultDocument),
    CachedDocument(InlineQueryResultCachedDocument),
    Location(InlineQueryResultLocation),
    Venue(InlineQueryResultVenue),
    Contact(InlineQueryResultContact),
    Game(InlineQueryResultGame),
}

/// Inline query result article (text-based result)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultArticle {
    pub id: String,
    pub title: String,
    pub input_message_content: InputMessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_url: Option<String>,
}

/// Inline query result photo
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultPhoto {
    pub id: String,
    pub photo_url: String,
    pub thumb_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

/// Inline query result sticker (cached on Telegram servers)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultCachedSticker {
    pub id: String,
    pub sticker_file_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
}

/// Inline query result GIF animation (from URL)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultGif {
    pub id: String,
    pub gif_url: String,
    pub thumbnail_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gif_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gif_height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gif_duration: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
}

/// Inline query result GIF animation (cached on Telegram servers)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultCachedGif {
    pub id: String,
    pub gif_file_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
}

/// Inline query result MP4 animation (from URL)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultMpeg4Gif {
    pub id: String,
    pub mpeg4_url: String,
    pub thumbnail_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpeg4_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpeg4_height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpeg4_duration: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
}

/// Inline query result MP4 animation (cached on Telegram servers)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultCachedMpeg4Gif {
    pub id: String,
    pub mpeg4_file_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
}

/// Inline query result video (from URL)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultVideo {
    pub id: String,
    pub video_url: String,
    pub mime_type: String,
    pub thumbnail_url: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_duration: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Inline query result video (cached on Telegram servers)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultCachedVideo {
    pub id: String,
    pub video_file_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
}

/// Inline query result audio (from URL)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultAudio {
    pub id: String,
    pub audio_url: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub performer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_duration: Option<u32>,
}

/// Inline query result audio (cached on Telegram servers)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultCachedAudio {
    pub id: String,
    pub audio_file_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
}

/// Inline query result voice (from URL)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultVoice {
    pub id: String,
    pub voice_url: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_duration: Option<u32>,
}

/// Inline query result voice (cached on Telegram servers)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultCachedVoice {
    pub id: String,
    pub voice_file_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
}

/// Inline query result document (from URL)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultDocument {
    pub id: String,
    pub title: String,
    pub document_url: String,
    pub mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_height: Option<u32>,
}

/// Inline query result document (cached on Telegram servers)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultCachedDocument {
    pub id: String,
    pub title: String,
    pub document_file_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
}

/// Inline query result photo (cached on Telegram servers)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultCachedPhoto {
    pub id: String,
    pub photo_file_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
}

/// Inline query result location
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultLocation {
    pub id: String,
    pub latitude: f64,
    pub longitude: f64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub horizontal_accuracy: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_period: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proximity_alert_radius: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_height: Option<u32>,
}

/// Inline query result venue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultVenue {
    pub id: String,
    pub latitude: f64,
    pub longitude: f64,
    pub title: String,
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foursquare_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foursquare_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub google_place_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub google_place_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_height: Option<u32>,
}

/// Inline query result contact
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultContact {
    pub id: String,
    pub phone_number: String,
    pub first_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vcard: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_height: Option<u32>,
}

/// Inline query result game
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryResultGame {
    pub id: String,
    pub game_short_name: String,
}

/// Input message content for inline results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputMessageContent {
    pub message_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
}

impl SendMessageCommand {
    /// Create a simple text message
    pub fn new(chat_id: i64, text: impl Into<String>) -> Self {
        Self {
            chat_id,
            text: text.into(),
            parse_mode: None,
            reply_to_message_id: None,
            reply_markup: None,
            message_thread_id: None,
        }
    }

    /// Add inline keyboard buttons
    pub fn with_buttons(mut self, markup: InlineKeyboardMarkup) -> Self {
        self.reply_markup = Some(markup);
        self
    }

    /// Set parse mode
    pub fn with_parse_mode(mut self, mode: ParseMode) -> Self {
        self.parse_mode = Some(mode);
        self
    }

    /// Reply to a specific message
    pub fn reply_to(mut self, message_id: i32) -> Self {
        self.reply_to_message_id = Some(message_id);
        self
    }

    /// Send message to a specific forum topic
    pub fn in_topic(mut self, thread_id: i32) -> Self {
        self.message_thread_id = Some(thread_id);
        self
    }
}

/// Create a forum topic
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateForumTopicCommand {
    pub chat_id: i64,
    pub name: String,
    /// Color of the topic icon in RGB format (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<i32>,
    /// Unique identifier of the custom emoji shown as the topic icon (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_custom_emoji_id: Option<String>,
}

/// Edit a forum topic
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditForumTopicCommand {
    pub chat_id: i64,
    pub message_thread_id: i32,
    /// New topic name (optional, 1-128 characters)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// New unique identifier of the custom emoji shown as the topic icon (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_custom_emoji_id: Option<String>,
}

/// Close a forum topic
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloseForumTopicCommand {
    pub chat_id: i64,
    pub message_thread_id: i32,
}

/// Reopen a forum topic
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReopenForumTopicCommand {
    pub chat_id: i64,
    pub message_thread_id: i32,
}

/// Delete a forum topic
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteForumTopicCommand {
    pub chat_id: i64,
    pub message_thread_id: i32,
}

/// Unpin all messages in a forum topic
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnpinAllForumTopicMessagesCommand {
    pub chat_id: i64,
    pub message_thread_id: i32,
}

/// Edit the name of the 'General' topic in a forum supergroup chat
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditGeneralForumTopicCommand {
    pub chat_id: i64,
    pub name: String,
}

/// Close the 'General' topic in a forum supergroup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloseGeneralForumTopicCommand {
    pub chat_id: i64,
}

/// Reopen the 'General' topic in a forum supergroup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReopenGeneralForumTopicCommand {
    pub chat_id: i64,
}

/// Hide the 'General' topic in a forum supergroup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HideGeneralForumTopicCommand {
    pub chat_id: i64,
}

/// Unhide the 'General' topic in a forum supergroup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnhideGeneralForumTopicCommand {
    pub chat_id: i64,
}

/// Unpin all messages in a general forum topic
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnpinAllGeneralForumTopicMessagesCommand {
    pub chat_id: i64,
}
