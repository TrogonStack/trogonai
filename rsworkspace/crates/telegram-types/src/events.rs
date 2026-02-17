//! Events published from Telegram bot to NATS

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::chat::{Chat, ChatInviteLink, ChatMember, FileInfo, ForumTopic, Message, MessageEntity, PhotoSize, User, ShippingAddress, OrderInfo, SuccessfulPayment};

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
    /// Message entities (mentions, hashtags, URLs, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entities: Option<Vec<MessageEntity>>,
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

/// Forum topic created event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForumTopicCreatedEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub forum_topic: ForumTopic,
}

/// Forum topic edited event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForumTopicEditedEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub name: Option<String>,
    pub icon_custom_emoji_id: Option<String>,
}

/// Forum topic closed event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForumTopicClosedEvent {
    pub metadata: EventMetadata,
    pub message: Message,
}

/// Forum topic reopened event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForumTopicReopenedEvent {
    pub metadata: EventMetadata,
    pub message: Message,
}

/// General forum topic hidden event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralForumTopicHiddenEvent {
    pub metadata: EventMetadata,
    pub message: Message,
}

/// General forum topic unhidden event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralForumTopicUnhiddenEvent {
    pub metadata: EventMetadata,
    pub message: Message,
}

/// Chat member updated event (join/leave/ban/promote/etc)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMemberUpdatedEvent {
    pub metadata: EventMetadata,
    pub chat: Chat,
    pub from: User,
    pub old_chat_member: ChatMember,
    pub new_chat_member: ChatMember,
    /// Date the change was done
    pub date: i64,
    /// Chat invite link used by the user to join the chat
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite_link: Option<ChatInviteLink>,
    /// True if the user joined via a join request
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via_join_request: Option<bool>,
    /// True if the user joined via a chat folder invite link
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via_chat_folder_invite_link: Option<bool>,
}

/// My chat member updated event (bot's status changed in a chat)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MyChatMemberUpdatedEvent {
    pub metadata: EventMetadata,
    pub chat: Chat,
    pub from: User,
    pub old_chat_member: ChatMember,
    pub new_chat_member: ChatMember,
    /// Date the change was done
    pub date: i64,
    /// Chat invite link used by the bot to join the chat
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite_link: Option<ChatInviteLink>,
    /// True if the bot joined via a join request
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via_join_request: Option<bool>,
    /// True if the bot joined via a chat folder invite link
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via_chat_folder_invite_link: Option<bool>,
}

/// Pre-checkout query event (before payment is confirmed)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreCheckoutQueryEvent {
    pub metadata: EventMetadata,
    /// Unique query identifier
    pub pre_checkout_query_id: String,
    /// User who sent the query
    pub from: User,
    /// Three-letter ISO 4217 currency code
    pub currency: String,
    /// Total price in the smallest units of the currency
    pub total_amount: i64,
    /// Bot specified invoice payload
    pub invoice_payload: String,
    /// Identifier of the shipping option chosen by the user
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shipping_option_id: Option<String>,
    /// Order info provided by the user
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_info: Option<OrderInfo>,
}

/// Shipping query event (for physical goods)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShippingQueryEvent {
    pub metadata: EventMetadata,
    /// Unique query identifier
    pub shipping_query_id: String,
    /// User who sent the query
    pub from: User,
    /// Bot specified invoice payload
    pub invoice_payload: String,
    /// User specified shipping address
    pub shipping_address: ShippingAddress,
}

/// Successful payment event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessfulPaymentEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub payment: SuccessfulPayment,
}
