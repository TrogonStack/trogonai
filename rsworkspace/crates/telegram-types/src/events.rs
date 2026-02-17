//! Events published from Telegram bot to NATS

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::chat::{Chat, FileInfo, Message, PhotoSize, User};

/// Base event metadata shared across all events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMetadata {
    /// Unique event ID
    pub event_id: Uuid,
    /// Session ID (format: tg-{chat_type}-{chat_id}[-{user_id}])
    pub session_id: String,
    /// Event timestamp
    pub timestamp: DateTime<Utc>,
    /// Update ID from Telegram
    pub update_id: i64,
}

/// Text message event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageTextEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub text: String,
}

/// Photo message event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagePhotoEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub photo: Vec<PhotoSize>,
    pub caption: Option<String>,
}

/// Video message event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageVideoEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub video: FileInfo,
    pub caption: Option<String>,
}

/// Audio message event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageAudioEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub audio: FileInfo,
    pub caption: Option<String>,
}

/// Document message event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageDocumentEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub document: FileInfo,
    pub caption: Option<String>,
}

/// Voice message event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageVoiceEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub voice: FileInfo,
}

/// Callback query event (button click)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallbackQueryEvent {
    pub metadata: EventMetadata,
    pub callback_query_id: String,
    pub from: User,
    pub chat: Chat,
    pub message_id: Option<i32>,
    pub data: String,
}

/// Command event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub command: String,
    pub args: Vec<String>,
}

/// Inline query event (when user types @bot query in any chat)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineQueryEvent {
    pub metadata: EventMetadata,
    pub inline_query_id: String,
    pub from: User,
    pub query: String,
    pub offset: String,
    pub chat_type: Option<String>,
    pub location: Option<Location>,
}

/// User location (for location-based inline queries)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub longitude: f64,
    pub latitude: f64,
}

/// Chosen inline result event (when user selects an inline result)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChosenInlineResultEvent {
    pub metadata: EventMetadata,
    pub result_id: String,
    pub from: User,
    pub query: String,
    pub inline_message_id: Option<String>,
}

impl EventMetadata {
    /// Create new event metadata
    pub fn new(session_id: String, update_id: i64) -> Self {
        Self {
            event_id: Uuid::new_v4(),
            session_id,
            timestamp: Utc::now(),
            update_id,
        }
    }
}
