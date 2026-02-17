//! Commands sent from agents to Telegram bot via NATS

use serde::{Deserialize, Serialize};

use crate::chat::{BotCommand, BotCommandScope, ChatAdministratorRights, ChatPermissions, InlineKeyboardMarkup, InputSticker, LabeledPrice, MaskPosition, ShippingOption, StickerSet};

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

/// Forward a message from one chat to another
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardMessageCommand {
    /// Destination chat ID
    pub chat_id: i64,
    /// Source chat ID
    pub from_chat_id: i64,
    /// Message ID in the source chat
    pub message_id: i32,
    /// Forum topic thread ID in the destination chat (forum supergroups only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_notification: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protect_content: Option<bool>,
}

/// Copy a message from one chat to another (no forward attribution)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyMessageCommand {
    /// Destination chat ID
    pub chat_id: i64,
    /// Source chat ID
    pub from_chat_id: i64,
    /// Message ID in the source chat
    pub message_id: i32,
    /// Forum topic thread ID in the destination chat (forum supergroups only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_notification: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protect_content: Option<bool>,
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

/// Send a video
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendVideoCommand {
    pub chat_id: i64,
    /// file_id or URL of the video
    pub video: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_streaming: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
}

/// Send an audio file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendAudioCommand {
    pub chat_id: i64,
    /// file_id or URL of the audio
    pub audio: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub performer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
}

/// Send a document (file)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendDocumentCommand {
    pub chat_id: i64,
    /// file_id or URL of the document
    pub document: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
}

/// Send a voice message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendVoiceCommand {
    pub chat_id: i64,
    /// file_id or URL of the voice message (ogg encoded with OPUS)
    pub voice: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
}

/// An item in a media group
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputMediaItem {
    Photo {
        /// file_id or URL
        media: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parse_mode: Option<ParseMode>,
    },
    Video {
        media: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parse_mode: Option<ParseMode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        supports_streaming: Option<bool>,
    },
    Audio {
        media: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parse_mode: Option<ParseMode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        performer: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Document {
        media: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parse_mode: Option<ParseMode>,
    },
}

/// Send a group of media (2-10 items)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMediaGroupCommand {
    pub chat_id: i64,
    /// 2–10 media items (photo, video, audio, or document)
    pub media: Vec<InputMediaItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
}

/// Send a sticker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendStickerCommand {
    pub chat_id: i64,
    /// file_id of the sticker on Telegram servers
    pub sticker: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
}

/// Send an animation (GIF)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendAnimationCommand {
    pub chat_id: i64,
    /// file_id or URL of the animation
    pub animation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
}

/// Send a video note (short round video)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendVideoNoteCommand {
    pub chat_id: i64,
    /// file_id of the video note
    pub video_note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<u32>,
    /// Diameter of the video (must be equal to video width and height)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i32>,
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

/// Promote a user to administrator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromoteChatMemberCommand {
    pub chat_id: i64,
    pub user_id: i64,
    /// Administrator rights to grant
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rights: Option<ChatAdministratorRights>,
}

/// Restrict a user in a chat
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestrictChatMemberCommand {
    pub chat_id: i64,
    pub user_id: i64,
    /// New user permissions
    pub permissions: ChatPermissions,
    /// Date when restrictions will be lifted (Unix time, None = forever)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until_date: Option<i64>,
}

/// Ban a user from a chat
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BanChatMemberCommand {
    pub chat_id: i64,
    pub user_id: i64,
    /// Date when the user will be unbanned (Unix time, None = forever)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until_date: Option<i64>,
    /// True to delete all messages from the chat for the user
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoke_messages: Option<bool>,
}

/// Unban a user from a chat
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnbanChatMemberCommand {
    pub chat_id: i64,
    pub user_id: i64,
    /// True to allow the user to join the chat again
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only_if_banned: Option<bool>,
}

/// Set default chat permissions for all members
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetChatPermissionsCommand {
    pub chat_id: i64,
    pub permissions: ChatPermissions,
}

/// Set a custom title for an administrator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetChatAdministratorCustomTitleCommand {
    pub chat_id: i64,
    pub user_id: i64,
    pub custom_title: String,
}

/// Pin a message in a chat
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinChatMessageCommand {
    pub chat_id: i64,
    pub message_id: i32,
    /// True to pin without notification
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_notification: Option<bool>,
}

/// Unpin a message in a chat
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnpinChatMessageCommand {
    pub chat_id: i64,
    /// Message ID to unpin (None = unpin most recent)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<i32>,
}

/// Unpin all messages in a chat
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnpinAllChatMessagesCommand {
    pub chat_id: i64,
}

/// Set chat title
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetChatTitleCommand {
    pub chat_id: i64,
    pub title: String,
}

/// Set chat description
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetChatDescriptionCommand {
    pub chat_id: i64,
    pub description: String,
}

/// Get file information from Telegram
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetFileCommand {
    /// File identifier to get info about
    pub file_id: String,
    /// Optional request ID for tracking responses
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Download file from Telegram
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadFileCommand {
    /// File identifier to download
    pub file_id: String,
    /// Local path where file should be saved (relative to bot's download directory)
    pub destination_path: String,
    /// Optional request ID for tracking responses
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// File information response from getFile API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfoResponse {
    /// Original file_id from the request
    pub file_id: String,
    /// Unique identifier for this file
    pub file_unique_id: String,
    /// File size in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_size: Option<u64>,
    /// File path on Telegram servers (use for download)
    pub file_path: String,
    /// Full download URL for the file
    pub download_url: String,
    /// Optional request ID from the original request
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// File download completion response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDownloadResponse {
    /// Original file_id
    pub file_id: String,
    /// Local path where file was saved
    pub local_path: String,
    /// File size in bytes
    pub file_size: u64,
    /// True if download succeeded
    pub success: bool,
    /// Error message if download failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Optional request ID from the original request
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

// ============================================================================
// Bot Commands (Menu Setup)
// ============================================================================

/// Set bot commands (appears in bot menu)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetMyCommandsCommand {
    /// List of bot commands
    pub commands: Vec<BotCommand>,
    /// Scope for which commands apply
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<BotCommandScope>,
    /// Language code (e.g., "en", "es", "pt")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_code: Option<String>,
}

/// Delete bot commands
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteMyCommandsCommand {
    /// Scope for which to delete commands
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<BotCommandScope>,
    /// Language code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_code: Option<String>,
}

/// Get bot commands
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetMyCommandsCommand {
    /// Scope for which to get commands
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<BotCommandScope>,
    /// Language code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_code: Option<String>,
    /// Optional request ID for tracking
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Response with bot commands
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotCommandsResponse {
    /// List of bot commands
    pub commands: Vec<BotCommand>,
    /// Optional request ID from the original request
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

// ============================================================================
// Payments
// ============================================================================

/// Send invoice (payment request)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendInvoiceCommand {
    /// Target chat (must be private chat for invoices)
    pub chat_id: i64,
    /// Product name, 1-32 characters
    pub title: String,
    /// Product description, 1-255 characters
    pub description: String,
    /// Bot-defined invoice payload, 1-128 bytes
    pub payload: String,
    /// Payment provider token (from @BotFather or payment provider)
    pub provider_token: String,
    /// Three-letter ISO 4217 currency code (e.g., USD, EUR)
    pub currency: String,
    /// Price breakdown (at least one LabeledPrice required)
    pub prices: Vec<LabeledPrice>,
    /// Maximum accepted amount for tips in the smallest units
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tip_amount: Option<i64>,
    /// Suggested amounts of tips in the smallest units
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_tip_amounts: Option<Vec<i64>>,
    /// Unique deep-linking parameter for this invoice
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_parameter: Option<String>,
    /// JSON-serialized data about the invoice for payment provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_data: Option<String>,
    /// URL of the product photo (648x648px max)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photo_url: Option<String>,
    /// Photo size
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photo_size: Option<i32>,
    /// Photo width
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photo_width: Option<i32>,
    /// Photo height
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photo_height: Option<i32>,
    /// True if you require user's full name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub need_name: Option<bool>,
    /// True if you require user's phone number
    #[serde(skip_serializing_if = "Option::is_none")]
    pub need_phone_number: Option<bool>,
    /// True if you require user's email
    #[serde(skip_serializing_if = "Option::is_none")]
    pub need_email: Option<bool>,
    /// True if you require user's shipping address
    #[serde(skip_serializing_if = "Option::is_none")]
    pub need_shipping_address: Option<bool>,
    /// True if user's email should be sent to provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_email_to_provider: Option<bool>,
    /// True if user's phone number should be sent to provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_phone_number_to_provider: Option<bool>,
    /// True if final price depends on shipping method
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_flexible: Option<bool>,
    /// Disable notification
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_notification: Option<bool>,
    /// Protects the contents of the sent message from forwarding and saving
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protect_content: Option<bool>,
    /// Reply to message ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i32>,
    /// Inline keyboard markup
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
}

/// Answer pre-checkout query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerPreCheckoutQueryCommand {
    /// Unique query identifier
    pub pre_checkout_query_id: String,
    /// True if everything is alright
    pub ok: bool,
    /// Error message if ok is false (required when ok is false)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// Answer shipping query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerShippingQueryCommand {
    /// Unique query identifier
    pub shipping_query_id: String,
    /// True if delivery to the address is possible
    pub ok: bool,
    /// Available shipping options (required if ok is true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shipping_options: Option<Vec<ShippingOption>>,
    /// Error message if ok is false (required when ok is false)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

// ── Polls ─────────────────────────────────────────────────────────────────────

/// Poll type for sending
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PollKind {
    Regular,
    Quiz,
}

/// Send a poll
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendPollCommand {
    pub chat_id: i64,
    /// Poll question (1-300 characters)
    pub question: String,
    /// Answer options (2-10 items, 1-100 characters each)
    pub options: Vec<String>,
    /// True for anonymous poll (default true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_anonymous: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll_type: Option<PollKind>,
    /// True if poll allows multiple answers (regular polls only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allows_multiple_answers: Option<bool>,
    /// 0-based index of the correct option (quiz polls only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correct_option_id: Option<u8>,
    /// Text shown on incorrect answer (quiz polls only, 0-200 chars)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation_parse_mode: Option<ParseMode>,
    /// Seconds the poll will be active (5-600)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_period: Option<u16>,
    /// Unix timestamp when the poll closes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close_date: Option<i64>,
    /// Pass true to immediately close the poll
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_closed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
}

/// Stop a poll
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopPollCommand {
    pub chat_id: i64,
    /// Identifier of the original poll message
    pub message_id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
}

// ── Location / Venue / Contact ────────────────────────────────────────────────

/// Send a location
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendLocationCommand {
    pub chat_id: i64,
    pub latitude: f64,
    pub longitude: f64,
    /// Live location period in seconds (60-86400, or 0x7FFFFFFF for indefinite)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_period: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub horizontal_accuracy: Option<f64>,
    /// Heading direction in degrees (1-360)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading: Option<u16>,
    /// Proximity alert radius in meters (1-100000)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proximity_alert_radius: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
}

/// Send a venue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendVenueCommand {
    pub chat_id: i64,
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
    pub reply_to_message_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
}

/// Send a contact
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendContactCommand {
    pub chat_id: i64,
    pub phone_number: String,
    pub first_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vcard: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_markup: Option<InlineKeyboardMarkup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
}

// ── Sticker set management ────────────────────────────────────────────────────

/// Get sticker set info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetStickerSetCommand {
    /// Name of the sticker set
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Response to GetStickerSetCommand
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StickerSetResponse {
    pub sticker_set: StickerSet,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Upload a sticker file (returns file_id usable for creating sticker sets)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadStickerFileCommand {
    /// User ID of the sticker set owner
    pub user_id: i64,
    /// file_id of the sticker to upload
    pub sticker: String,
    /// Format: "static", "animated", or "video"
    pub format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Response to UploadStickerFileCommand
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadStickerFileResponse {
    /// file_id of the uploaded sticker file
    pub file_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Create a new sticker set
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateNewStickerSetCommand {
    /// User ID of the sticker set owner
    pub user_id: i64,
    /// Short name (1-64 chars, a-z 0-9 _, must end in _by_<bot_username>)
    pub name: String,
    /// Sticker set title (1-64 chars)
    pub title: String,
    /// List of stickers to be added to the set (1-50)
    pub stickers: Vec<InputSticker>,
    /// Kind of stickers in the set: "regular", "mask", or "custom_emoji"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sticker_type: Option<String>,
    /// True if custom emoji stickers can be used as chat boosts
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_repainting: Option<bool>,
}

/// Add a sticker to an existing set
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddStickerToSetCommand {
    /// User ID of the sticker set owner
    pub user_id: i64,
    /// Sticker set name
    pub name: String,
    /// Sticker to add
    pub sticker: InputSticker,
}

/// Move a sticker to a specific position in its set
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetStickerPositionInSetCommand {
    /// file_id of the sticker
    pub sticker: String,
    /// New zero-based position in the set
    pub position: u32,
}

/// Delete a sticker from its set
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteStickerFromSetCommand {
    /// file_id of the sticker to delete
    pub sticker: String,
}

/// Set the title of a sticker set
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetStickerSetTitleCommand {
    /// Sticker set name
    pub name: String,
    /// New title (1-64 chars)
    pub title: String,
}

/// Set the thumbnail of a sticker set
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetStickerSetThumbnailCommand {
    /// Sticker set name
    pub name: String,
    /// User ID of the sticker set owner
    pub user_id: i64,
    /// Format of the thumbnail: "static", "animated", or "video"
    pub format: String,
    /// file_id of the thumbnail (omit to remove)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
}

/// Delete an entire sticker set
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteStickerSetCommand {
    /// Sticker set name
    pub name: String,
}

/// Set the list of emoji associated with a sticker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetStickerEmojiListCommand {
    /// file_id of the sticker
    pub sticker: String,
    /// 1-20 emoji associated with the sticker
    pub emoji_list: Vec<String>,
}

/// Set search keywords for a sticker (regular and custom_emoji only)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetStickerKeywordsCommand {
    /// file_id of the sticker
    pub sticker: String,
    /// 0-20 keywords (total ≤ 64 chars)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub keywords: Vec<String>,
}

/// Set the mask position for a mask sticker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetStickerMaskPositionCommand {
    /// file_id of the sticker
    pub sticker: String,
    /// New mask position (omit to remove)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask_position: Option<MaskPosition>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T: Serialize + for<'de> Deserialize<'de>>(val: &T) {
        let json = serde_json::to_string(val).expect("serialize");
        let back: T = serde_json::from_str(&json).expect("deserialize");
        let json2 = serde_json::to_string(&back).expect("re-serialize");
        assert_eq!(json, json2, "roundtrip produced different JSON");
    }

    #[test]
    fn test_send_video_command_roundtrip() {
        let cmd = SendVideoCommand {
            chat_id: 100,
            video: "file_id_123".to_string(),
            caption: Some("Watch this".to_string()),
            parse_mode: Some(ParseMode::HTML),
            duration: Some(30),
            width: Some(1280),
            height: Some(720),
            supports_streaming: Some(true),
            reply_to_message_id: None,
            reply_markup: None,
            message_thread_id: None,
        };
        roundtrip(&cmd);
    }

    #[test]
    fn test_send_audio_command_roundtrip() {
        let cmd = SendAudioCommand {
            chat_id: 200,
            audio: "audio_file_id".to_string(),
            caption: None,
            parse_mode: None,
            duration: Some(180),
            performer: Some("Artist".to_string()),
            title: Some("Song".to_string()),
            reply_to_message_id: Some(5),
            reply_markup: None,
            message_thread_id: None,
        };
        roundtrip(&cmd);
    }

    #[test]
    fn test_send_document_command_roundtrip() {
        let cmd = SendDocumentCommand {
            chat_id: 300,
            document: "doc_file_id".to_string(),
            caption: Some("Here's the file".to_string()),
            parse_mode: Some(ParseMode::MarkdownV2),
            reply_to_message_id: None,
            reply_markup: None,
            message_thread_id: Some(7),
        };
        roundtrip(&cmd);
    }

    #[test]
    fn test_send_voice_command_roundtrip() {
        let cmd = SendVoiceCommand {
            chat_id: 400,
            voice: "voice_file_id".to_string(),
            caption: None,
            parse_mode: None,
            duration: Some(10),
            reply_to_message_id: None,
            reply_markup: None,
            message_thread_id: None,
        };
        roundtrip(&cmd);
    }

    #[test]
    fn test_input_media_item_variants_roundtrip() {
        let photo = InputMediaItem::Photo {
            media: "photo_id".to_string(),
            caption: Some("pic".to_string()),
            parse_mode: None,
        };
        roundtrip(&photo);

        let video = InputMediaItem::Video {
            media: "video_id".to_string(),
            caption: None,
            parse_mode: None,
            duration: Some(60),
            width: Some(1920),
            height: Some(1080),
            supports_streaming: Some(true),
        };
        roundtrip(&video);

        let audio = InputMediaItem::Audio {
            media: "audio_id".to_string(),
            caption: None,
            parse_mode: None,
            duration: Some(120),
            performer: Some("Band".to_string()),
            title: Some("Track".to_string()),
        };
        roundtrip(&audio);

        let doc = InputMediaItem::Document {
            media: "doc_id".to_string(),
            caption: None,
            parse_mode: None,
        };
        roundtrip(&doc);
    }

    #[test]
    fn test_send_media_group_command_roundtrip() {
        let cmd = SendMediaGroupCommand {
            chat_id: 500,
            media: vec![
                InputMediaItem::Photo { media: "p1".to_string(), caption: None, parse_mode: None },
                InputMediaItem::Video {
                    media: "v1".to_string(),
                    caption: Some("vid".to_string()),
                    parse_mode: Some(ParseMode::HTML),
                    duration: None,
                    width: None,
                    height: None,
                    supports_streaming: None,
                },
            ],
            reply_to_message_id: None,
            message_thread_id: None,
        };
        roundtrip(&cmd);
    }

    #[test]
    fn test_send_location_command_roundtrip() {
        let cmd = SendLocationCommand {
            chat_id: 600,
            latitude: 40.416775,
            longitude: -3.703790,
            live_period: Some(300),
            horizontal_accuracy: Some(10.5),
            heading: Some(270),
            proximity_alert_radius: Some(100),
            reply_to_message_id: None,
            reply_markup: None,
            message_thread_id: None,
        };
        roundtrip(&cmd);
    }

    #[test]
    fn test_send_venue_command_roundtrip() {
        let cmd = SendVenueCommand {
            chat_id: 700,
            latitude: 48.8566,
            longitude: 2.3522,
            title: "Louvre".to_string(),
            address: "Rue de Rivoli".to_string(),
            foursquare_id: None,
            foursquare_type: None,
            google_place_id: Some("ChIJ...".to_string()),
            google_place_type: Some("museum".to_string()),
            reply_to_message_id: None,
            reply_markup: None,
            message_thread_id: None,
        };
        roundtrip(&cmd);
    }

    #[test]
    fn test_send_contact_command_roundtrip() {
        let cmd = SendContactCommand {
            chat_id: 800,
            phone_number: "+1234567890".to_string(),
            first_name: "Alice".to_string(),
            last_name: Some("Smith".to_string()),
            vcard: None,
            reply_to_message_id: None,
            reply_markup: None,
            message_thread_id: None,
        };
        roundtrip(&cmd);
    }

    #[test]
    fn test_send_poll_command_roundtrip() {
        let cmd = SendPollCommand {
            chat_id: 900,
            question: "Favorite color?".to_string(),
            options: vec!["Red".to_string(), "Blue".to_string(), "Green".to_string()],
            is_anonymous: Some(false),
            poll_type: Some(PollKind::Regular),
            allows_multiple_answers: Some(true),
            correct_option_id: None,
            explanation: None,
            explanation_parse_mode: None,
            open_period: Some(60),
            close_date: None,
            is_closed: None,
            reply_to_message_id: None,
            reply_markup: None,
            message_thread_id: None,
        };
        roundtrip(&cmd);
    }

    #[test]
    fn test_send_quiz_poll_command_roundtrip() {
        let cmd = SendPollCommand {
            chat_id: 901,
            question: "Capital of France?".to_string(),
            options: vec!["London".to_string(), "Paris".to_string(), "Madrid".to_string()],
            is_anonymous: Some(true),
            poll_type: Some(PollKind::Quiz),
            allows_multiple_answers: None,
            correct_option_id: Some(1),
            explanation: Some("It's Paris!".to_string()),
            explanation_parse_mode: Some(ParseMode::HTML),
            open_period: None,
            close_date: None,
            is_closed: None,
            reply_to_message_id: None,
            reply_markup: None,
            message_thread_id: None,
        };
        roundtrip(&cmd);
    }

    #[test]
    fn test_stop_poll_command_roundtrip() {
        let cmd = StopPollCommand {
            chat_id: 1000,
            message_id: 55,
            reply_markup: None,
        };
        roundtrip(&cmd);
    }

    #[test]
    fn test_poll_kind_serde() {
        assert_eq!(serde_json::to_string(&PollKind::Regular).unwrap(), "\"regular\"");
        assert_eq!(serde_json::to_string(&PollKind::Quiz).unwrap(), "\"quiz\"");
        let rt: PollKind = serde_json::from_str("\"quiz\"").unwrap();
        assert_eq!(rt, PollKind::Quiz);
    }

    #[test]
    fn test_parse_mode_serde() {
        assert_eq!(serde_json::to_string(&ParseMode::HTML).unwrap(), "\"HTML\"");
        assert_eq!(serde_json::to_string(&ParseMode::Markdown).unwrap(), "\"Markdown\"");
        assert_eq!(serde_json::to_string(&ParseMode::MarkdownV2).unwrap(), "\"MarkdownV2\"");
    }

    // ── ChatAction serde ──────────────────────────────────────────────────────

    #[test]
    fn test_chat_action_serde() {
        for (action, expected) in [
            (ChatAction::Typing, "\"typing\""),
            (ChatAction::UploadPhoto, "\"upload_photo\""),
            (ChatAction::RecordVideo, "\"record_video\""),
            (ChatAction::UploadDocument, "\"upload_document\""),
            (ChatAction::FindLocation, "\"find_location\""),
        ] {
            let json = serde_json::to_string(&action).unwrap();
            assert_eq!(json, expected);
            let back: ChatAction = serde_json::from_str(&json).unwrap();
            assert_eq!(back, action);
        }
    }

    // ── Core messaging commands ───────────────────────────────────────────────

    #[test]
    fn test_send_message_command_roundtrip() {
        let cmd = SendMessageCommand {
            chat_id: 123,
            text: "Hello!".to_string(),
            parse_mode: Some(ParseMode::Markdown),
            reply_to_message_id: Some(10),
            reply_markup: None,
            message_thread_id: None,
        };
        roundtrip(&cmd);
    }

    #[test]
    fn test_send_message_optional_fields_omitted() {
        let cmd = SendMessageCommand {
            chat_id: 1,
            text: "Hi".to_string(),
            parse_mode: None,
            reply_to_message_id: None,
            reply_markup: None,
            message_thread_id: None,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(!json.contains("parse_mode"));
        assert!(!json.contains("reply_to_message_id"));
        assert!(!json.contains("reply_markup"));
        assert!(!json.contains("message_thread_id"));
    }

    #[test]
    fn test_edit_message_command_roundtrip() {
        let cmd = EditMessageCommand {
            chat_id: 42,
            message_id: 7,
            text: "Updated text".to_string(),
            parse_mode: Some(ParseMode::HTML),
            reply_markup: None,
        };
        roundtrip(&cmd);
    }

    #[test]
    fn test_delete_message_command_roundtrip() {
        let cmd = DeleteMessageCommand { chat_id: 999, message_id: 55 };
        roundtrip(&cmd);
    }

    #[test]
    fn test_send_photo_command_roundtrip() {
        let cmd = SendPhotoCommand {
            chat_id: 100,
            photo: "AgACAgIAAxk".to_string(),
            caption: Some("Nice photo".to_string()),
            parse_mode: None,
            reply_to_message_id: None,
            message_thread_id: None,
        };
        roundtrip(&cmd);
    }

    // ── Action / interaction commands ─────────────────────────────────────────

    #[test]
    fn test_answer_callback_command_roundtrip() {
        let cmd = AnswerCallbackCommand {
            callback_query_id: "cq_abc".to_string(),
            text: Some("Done!".to_string()),
            show_alert: Some(false),
        };
        roundtrip(&cmd);
    }

    #[test]
    fn test_answer_callback_command_no_text() {
        let cmd = AnswerCallbackCommand {
            callback_query_id: "cq_xyz".to_string(),
            text: None,
            show_alert: None,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(!json.contains("text"));
        assert!(!json.contains("show_alert"));
    }

    #[test]
    fn test_send_chat_action_command_roundtrip() {
        let cmd = SendChatActionCommand {
            chat_id: 200,
            action: ChatAction::Typing,
            message_thread_id: None,
        };
        roundtrip(&cmd);
    }

    // ── StreamMessageCommand ──────────────────────────────────────────────────

    #[test]
    fn test_stream_message_command_roundtrip() {
        let cmd = StreamMessageCommand {
            chat_id: 300,
            message_id: Some(15),
            text: "Partial response...".to_string(),
            parse_mode: Some(ParseMode::Markdown),
            is_final: false,
            session_id: Some("tg-private-300".to_string()),
            message_thread_id: None,
        };
        roundtrip(&cmd);
    }

    #[test]
    fn test_stream_message_command_final() {
        let cmd = StreamMessageCommand {
            chat_id: 300,
            message_id: Some(15),
            text: "Complete response.".to_string(),
            parse_mode: None,
            is_final: true,
            session_id: None,
            message_thread_id: None,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"is_final\":true"));
        assert!(!json.contains("\"session_id\""));
    }

    // ── AnswerInlineQueryCommand + InlineQueryResult ──────────────────────────

    #[test]
    fn test_answer_inline_query_command_roundtrip() {
        let cmd = AnswerInlineQueryCommand {
            inline_query_id: "iq_1".to_string(),
            results: vec![
                InlineQueryResult::Article(InlineQueryResultArticle {
                    id: "r1".to_string(),
                    title: "Result Title".to_string(),
                    input_message_content: InputMessageContent {
                        message_text: "The answer".to_string(),
                        parse_mode: None,
                    },
                    description: Some("Brief description".to_string()),
                    thumb_url: None,
                }),
            ],
            cache_time: Some(300),
            is_personal: Some(true),
            next_offset: None,
        };
        roundtrip(&cmd);
    }

    #[test]
    fn test_inline_query_result_article_has_type_tag() {
        let result = InlineQueryResult::Article(InlineQueryResultArticle {
            id: "1".to_string(),
            title: "T".to_string(),
            input_message_content: InputMessageContent { message_text: "M".to_string(), parse_mode: None },
            description: None,
            thumb_url: None,
        });
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"type\":\"article\""));
    }

    #[test]
    fn test_inline_query_result_cached_sticker_roundtrip() {
        let result = InlineQueryResult::CachedSticker(InlineQueryResultCachedSticker {
            id: "s1".to_string(),
            sticker_file_id: "stk_file_abc".to_string(),
            reply_markup: None,
        });
        roundtrip(&result);
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"type\":\"cached_sticker\""));
    }

    #[test]
    fn test_input_message_content_roundtrip() {
        let content = InputMessageContent {
            message_text: "Test message".to_string(),
            parse_mode: Some(ParseMode::MarkdownV2),
        };
        roundtrip(&content);
    }
}
