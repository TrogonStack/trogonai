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
