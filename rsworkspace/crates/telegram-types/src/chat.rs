//! Telegram chat, user, and message types

use serde::{Deserialize, Serialize};

/// Telegram chat type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatType {
    Private,
    Group,
    Supergroup,
    Channel,
}

/// Telegram chat
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type")]
    pub chat_type: ChatType,
    pub title: Option<String>,
    pub username: Option<String>,
}

/// Telegram user
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub is_bot: bool,
    pub first_name: String,
    pub last_name: Option<String>,
    pub username: Option<String>,
    pub language_code: Option<String>,
}

/// Telegram message metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub message_id: i32,
    pub date: i64,
    pub chat: Chat,
    pub from: Option<User>,
    /// Unique identifier of a message thread (topic) to which the message belongs; for supergroups only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i32>,
    /// True, if the message is sent to a forum topic
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_topic_message: Option<bool>,
}

/// File information for media messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub file_id: String,
    pub file_unique_id: String,
    pub file_size: Option<u64>,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
}

/// Photo size information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotoSize {
    pub file_id: String,
    pub file_unique_id: String,
    pub width: u32,
    pub height: u32,
    pub file_size: Option<u64>,
}

/// Inline keyboard button
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineKeyboardButton {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Inline keyboard markup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineKeyboardMarkup {
    pub inline_keyboard: Vec<Vec<InlineKeyboardButton>>,
}

impl InlineKeyboardMarkup {
    /// Create a new inline keyboard with rows of buttons
    pub fn new(rows: Vec<Vec<InlineKeyboardButton>>) -> Self {
        Self {
            inline_keyboard: rows,
        }
    }
}

/// Forum topic information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForumTopic {
    /// Unique identifier of the forum topic
    pub message_thread_id: i32,
    /// Name of the topic
    pub name: String,
    /// Color of the topic icon in RGB format
    pub icon_color: i32,
    /// Unique identifier of the custom emoji shown as the topic icon
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_custom_emoji_id: Option<String>,
}
