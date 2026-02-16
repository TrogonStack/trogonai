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
}
