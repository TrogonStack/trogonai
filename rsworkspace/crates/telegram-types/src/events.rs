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

/// Location message event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageLocationEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub longitude: f64,
    pub latitude: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub horizontal_accuracy: Option<f64>,
    /// Live location period in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_period: Option<u32>,
    /// Heading direction in degrees (1-360), live locations only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading: Option<u16>,
    /// Proximity alert radius in meters, live locations only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proximity_alert_radius: Option<u32>,
}

/// Venue message event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageVenueEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub longitude: f64,
    pub latitude: f64,
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
}

/// Contact message event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageContactEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub phone_number: String,
    pub first_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_name: Option<String>,
    /// Telegram user ID of the contact, if known
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vcard: Option<String>,
}

/// Sticker format (encoding)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StickerFormat {
    /// Static `.webp` sticker
    Static,
    /// Animated `.tgs` sticker
    Animated,
    /// Video `.webm` sticker
    Video,
}

/// Sticker kind (presentation context)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StickerKind {
    Regular,
    Mask,
    CustomEmoji,
}

/// Sticker message event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageStickerEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub file_id: String,
    pub file_unique_id: String,
    pub file_size: Option<u64>,
    pub width: u32,
    pub height: u32,
    pub format: StickerFormat,
    pub kind: StickerKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub set_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<PhotoSize>,
}

/// Animation (GIF) message event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageAnimationEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub animation: FileInfo,
    pub width: u32,
    pub height: u32,
    /// Duration in seconds
    pub duration: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<PhotoSize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

/// Video note message event (short round video)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageVideoNoteEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub video_note: FileInfo,
    /// Duration in seconds
    pub duration: u32,
    /// Diameter of the video
    pub length: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<PhotoSize>,
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

/// Poll type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PollType {
    Regular,
    Quiz,
}

/// A single poll answer option with its current vote count
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollOption {
    pub text: String,
    pub voter_count: u32,
}

/// Poll message event — poll sent inside a chat message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagePollEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub poll_id: String,
    pub question: String,
    pub options: Vec<PollOption>,
    pub total_voter_count: u32,
    pub is_closed: bool,
    pub is_anonymous: bool,
    pub poll_type: PollType,
    pub allows_multiple_answers: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correct_option_id: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    /// Open period in seconds (auto-close timer)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_period: Option<u32>,
}

/// Standalone poll update — sent when a poll's state changes (votes updated, closed, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollUpdateEvent {
    pub metadata: EventMetadata,
    pub poll_id: String,
    pub question: String,
    pub options: Vec<PollOption>,
    pub total_voter_count: u32,
    pub is_closed: bool,
    pub is_anonymous: bool,
    pub poll_type: PollType,
    pub allows_multiple_answers: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correct_option_id: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
}

/// Poll answer event — sent when a user votes in a non-anonymous poll
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollAnswerEvent {
    pub metadata: EventMetadata,
    pub poll_id: String,
    /// Indices of the chosen options (empty = vote retracted)
    pub option_ids: Vec<u8>,
    /// User ID of the voter (if not anonymous)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voter_user_id: Option<i64>,
    /// Chat ID when vote is cast anonymously via a chat
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voter_chat_id: Option<i64>,
}

/// Successful payment event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessfulPaymentEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    pub payment: SuccessfulPayment,
}

/// Edited message event (user edited a previously sent message)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditedMessageEvent {
    pub metadata: EventMetadata,
    pub message: Message,
    /// New text content (if text message was edited)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_text: Option<String>,
    /// New caption (if media message caption was edited)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_caption: Option<String>,
    /// Updated message entities
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entities: Option<Vec<MessageEntity>>,
}

/// Chat join request event (user requested to join a group/channel with approval required)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatJoinRequestEvent {
    pub metadata: EventMetadata,
    /// Chat the user wants to join
    pub chat: Chat,
    /// User who sent the request
    pub from: User,
    /// Date the request was sent (Unix timestamp)
    pub date: i64,
    /// User's bio (if set)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    /// Invite link used by the user (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite_link: Option<ChatInviteLink>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{Chat, ChatType, Message as TgMessage};

    fn test_metadata() -> EventMetadata {
        EventMetadata::new("tg-private-123".to_string(), 1)
    }

    fn test_message() -> TgMessage {
        TgMessage {
            message_id: 42,
            date: 0,
            chat: Chat { id: 123, chat_type: ChatType::Private, title: None, username: None },
            from: None,
            message_thread_id: None,
            is_topic_message: None,
        }
    }

    fn roundtrip<T: Serialize + for<'de> Deserialize<'de>>(val: &T) {
        let json = serde_json::to_string(val).expect("serialize");
        let back: T = serde_json::from_str(&json).expect("deserialize");
        let json2 = serde_json::to_string(&back).expect("re-serialize");
        assert_eq!(json, json2, "roundtrip produced different JSON");
    }

    #[test]
    fn test_poll_type_serde() {
        assert_eq!(serde_json::to_string(&PollType::Regular).unwrap(), "\"regular\"");
        assert_eq!(serde_json::to_string(&PollType::Quiz).unwrap(), "\"quiz\"");
        let rt: PollType = serde_json::from_str("\"regular\"").unwrap();
        assert_eq!(rt, PollType::Regular);
    }

    #[test]
    fn test_message_poll_event_roundtrip() {
        let event = MessagePollEvent {
            metadata: test_metadata(),
            message: test_message(),
            poll_id: "poll123".to_string(),
            question: "Best language?".to_string(),
            options: vec![
                PollOption { text: "Rust".to_string(), voter_count: 10 },
                PollOption { text: "Python".to_string(), voter_count: 5 },
            ],
            total_voter_count: 15,
            is_closed: false,
            is_anonymous: true,
            poll_type: PollType::Regular,
            allows_multiple_answers: false,
            correct_option_id: None,
            explanation: None,
            open_period: None,
        };
        roundtrip(&event);
    }

    #[test]
    fn test_poll_update_event_roundtrip() {
        let event = PollUpdateEvent {
            metadata: test_metadata(),
            poll_id: "p1".to_string(),
            question: "Quiz?".to_string(),
            options: vec![
                PollOption { text: "A".to_string(), voter_count: 1 },
                PollOption { text: "B".to_string(), voter_count: 0 },
            ],
            total_voter_count: 1,
            is_closed: true,
            is_anonymous: false,
            poll_type: PollType::Quiz,
            allows_multiple_answers: false,
            correct_option_id: Some(0),
            explanation: Some("Because A".to_string()),
        };
        roundtrip(&event);
    }

    #[test]
    fn test_poll_answer_event_roundtrip() {
        let event = PollAnswerEvent {
            metadata: test_metadata(),
            poll_id: "p2".to_string(),
            option_ids: vec![0, 2],
            voter_user_id: Some(999),
            voter_chat_id: None,
        };
        roundtrip(&event);

        let anon = PollAnswerEvent {
            metadata: test_metadata(),
            poll_id: "p3".to_string(),
            option_ids: vec![],
            voter_user_id: None,
            voter_chat_id: Some(-100123456),
        };
        roundtrip(&anon);
    }

    #[test]
    fn test_message_location_event_roundtrip() {
        let event = MessageLocationEvent {
            metadata: test_metadata(),
            message: test_message(),
            longitude: -3.703790,
            latitude: 40.416775,
            horizontal_accuracy: Some(15.0),
            live_period: Some(300),
            heading: Some(180),
            proximity_alert_radius: Some(500),
        };
        roundtrip(&event);
    }

    #[test]
    fn test_message_venue_event_roundtrip() {
        let event = MessageVenueEvent {
            metadata: test_metadata(),
            message: test_message(),
            longitude: 2.347,
            latitude: 48.859,
            title: "Eiffel Tower".to_string(),
            address: "Champ de Mars".to_string(),
            foursquare_id: Some("abc123".to_string()),
            foursquare_type: None,
            google_place_id: None,
            google_place_type: None,
        };
        roundtrip(&event);
    }

    #[test]
    fn test_message_contact_event_roundtrip() {
        let event = MessageContactEvent {
            metadata: test_metadata(),
            message: test_message(),
            phone_number: "+34600000000".to_string(),
            first_name: "Jorge".to_string(),
            last_name: Some("Garcia".to_string()),
            user_id: Some(42),
            vcard: None,
        };
        roundtrip(&event);
    }

    // ── EventMetadata::new() ──────────────────────────────────────────────────

    #[test]
    fn test_event_metadata_new() {
        let meta = EventMetadata::new("tg-private-99".to_string(), 42);
        assert_eq!(meta.session_id, "tg-private-99");
        assert_eq!(meta.update_id, 42);
        // event_id should be a valid UUID (non-zero)
        assert_ne!(meta.event_id.to_string(), "00000000-0000-0000-0000-000000000000");
    }

    // ── StickerFormat / StickerKind serde ─────────────────────────────────────

    #[test]
    fn test_sticker_format_serde() {
        for (v, e) in [
            (StickerFormat::Static, "\"static\""),
            (StickerFormat::Animated, "\"animated\""),
            (StickerFormat::Video, "\"video\""),
        ] {
            let json = serde_json::to_string(&v).unwrap();
            assert_eq!(json, e);
            let back: StickerFormat = serde_json::from_str(&json).unwrap();
            assert_eq!(back, v);
        }
    }

    #[test]
    fn test_sticker_kind_serde() {
        for (v, e) in [
            (StickerKind::Regular, "\"regular\""),
            (StickerKind::Mask, "\"mask\""),
            (StickerKind::CustomEmoji, "\"custom_emoji\""),
        ] {
            let json = serde_json::to_string(&v).unwrap();
            assert_eq!(json, e);
            let back: StickerKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, v);
        }
    }

    // ── Message-based events (text, photo, video, audio, document, voice) ─────

    #[test]
    fn test_message_text_event_roundtrip() {
        let event = MessageTextEvent {
            metadata: test_metadata(),
            message: test_message(),
            text: "Hello world".to_string(),
            entities: None,
        };
        roundtrip(&event);
    }

    #[test]
    fn test_message_photo_event_roundtrip() {
        let event = MessagePhotoEvent {
            metadata: test_metadata(),
            message: test_message(),
            photo: vec![
                PhotoSize { file_id: "f1".to_string(), file_unique_id: "u1".to_string(), width: 320, height: 240, file_size: None },
            ],
            caption: Some("A photo".to_string()),
        };
        roundtrip(&event);
    }

    #[test]
    fn test_message_video_event_roundtrip() {
        let event = MessageVideoEvent {
            metadata: test_metadata(),
            message: test_message(),
            video: FileInfo { file_id: "v1".to_string(), file_unique_id: "vu1".to_string(), file_size: Some(1024), file_name: Some("clip.mp4".to_string()), mime_type: Some("video/mp4".to_string()) },
            caption: None,
        };
        roundtrip(&event);
    }

    #[test]
    fn test_message_audio_event_roundtrip() {
        let event = MessageAudioEvent {
            metadata: test_metadata(),
            message: test_message(),
            audio: FileInfo { file_id: "a1".to_string(), file_unique_id: "au1".to_string(), file_size: None, file_name: Some("track.mp3".to_string()), mime_type: Some("audio/mpeg".to_string()) },
            caption: None,
        };
        roundtrip(&event);
    }

    #[test]
    fn test_message_document_event_roundtrip() {
        let event = MessageDocumentEvent {
            metadata: test_metadata(),
            message: test_message(),
            document: FileInfo { file_id: "d1".to_string(), file_unique_id: "du1".to_string(), file_size: Some(2048), file_name: Some("doc.pdf".to_string()), mime_type: Some("application/pdf".to_string()) },
            caption: Some("See attached".to_string()),
        };
        roundtrip(&event);
    }

    #[test]
    fn test_message_voice_event_roundtrip() {
        let event = MessageVoiceEvent {
            metadata: test_metadata(),
            message: test_message(),
            voice: FileInfo { file_id: "vo1".to_string(), file_unique_id: "vou1".to_string(), file_size: Some(512), file_name: None, mime_type: Some("audio/ogg".to_string()) },
        };
        roundtrip(&event);
    }

    #[test]
    fn test_message_sticker_event_roundtrip() {
        let event = MessageStickerEvent {
            metadata: test_metadata(),
            message: test_message(),
            file_id: "stk1".to_string(),
            file_unique_id: "stku1".to_string(),
            file_size: Some(256),
            width: 512,
            height: 512,
            format: StickerFormat::Static,
            kind: StickerKind::Regular,
            emoji: Some("😀".to_string()),
            set_name: Some("AnimatedStickers".to_string()),
            thumbnail: None,
        };
        roundtrip(&event);
    }

    #[test]
    fn test_message_animation_event_roundtrip() {
        let event = MessageAnimationEvent {
            metadata: test_metadata(),
            message: test_message(),
            animation: FileInfo { file_id: "gif1".to_string(), file_unique_id: "gifu1".to_string(), file_size: None, file_name: Some("anim.gif".to_string()), mime_type: Some("image/gif".to_string()) },
            width: 480,
            height: 270,
            duration: 3,
            thumbnail: None,
            caption: None,
        };
        roundtrip(&event);
    }

    #[test]
    fn test_message_video_note_event_roundtrip() {
        let event = MessageVideoNoteEvent {
            metadata: test_metadata(),
            message: test_message(),
            video_note: FileInfo { file_id: "vn1".to_string(), file_unique_id: "vnu1".to_string(), file_size: Some(4096), file_name: None, mime_type: None },
            duration: 7,
            length: 240,
            thumbnail: None,
        };
        roundtrip(&event);
    }

    // ── Interaction events ────────────────────────────────────────────────────

    #[test]
    fn test_callback_query_event_roundtrip() {
        let event = CallbackQueryEvent {
            metadata: test_metadata(),
            callback_query_id: "cq123".to_string(),
            from: User { id: 10, is_bot: false, first_name: "Alice".to_string(), last_name: None, username: None, language_code: None },
            chat: Chat { id: 999, chat_type: ChatType::Private, title: None, username: None },
            message_id: Some(5),
            data: "button:ok".to_string(),
        };
        roundtrip(&event);
    }

    #[test]
    fn test_command_event_roundtrip() {
        let event = CommandEvent {
            metadata: test_metadata(),
            message: test_message(),
            command: "start".to_string(),
            args: vec!["arg1".to_string(), "arg2".to_string()],
        };
        roundtrip(&event);
    }

    #[test]
    fn test_command_event_no_args() {
        let event = CommandEvent {
            metadata: test_metadata(),
            message: test_message(),
            command: "help".to_string(),
            args: vec![],
        };
        roundtrip(&event);
    }

    #[test]
    fn test_inline_query_event_roundtrip() {
        let event = InlineQueryEvent {
            metadata: test_metadata(),
            inline_query_id: "iq1".to_string(),
            from: User { id: 20, is_bot: false, first_name: "Bob".to_string(), last_name: None, username: Some("bob".to_string()), language_code: Some("es".to_string()) },
            query: "search term".to_string(),
            offset: "".to_string(),
            chat_type: Some("private".to_string()),
            location: Some(Location { longitude: -3.7, latitude: 40.4 }),
        };
        roundtrip(&event);
    }

    #[test]
    fn test_chosen_inline_result_event_roundtrip() {
        let event = ChosenInlineResultEvent {
            metadata: test_metadata(),
            result_id: "r1".to_string(),
            from: User { id: 30, is_bot: false, first_name: "Carol".to_string(), last_name: None, username: None, language_code: None },
            query: "my query".to_string(),
            inline_message_id: Some("im123".to_string()),
        };
        roundtrip(&event);
    }

    // ── Chat member events ────────────────────────────────────────────────────

    #[test]
    fn test_chat_member_updated_event_roundtrip() {
        use crate::chat::{ChatMember, ChatMemberStatus};
        let member = |status| ChatMember {
            user: User { id: 50, is_bot: false, first_name: "Dave".to_string(), last_name: None, username: None, language_code: None },
            status,
            until_date: None,
            is_anonymous: None,
            custom_title: None,
        };
        let event = ChatMemberUpdatedEvent {
            metadata: test_metadata(),
            chat: Chat { id: -100999, chat_type: ChatType::Group, title: Some("G".to_string()), username: None },
            from: User { id: 1, is_bot: true, first_name: "Bot".to_string(), last_name: None, username: None, language_code: None },
            old_chat_member: member(ChatMemberStatus::Member),
            new_chat_member: member(ChatMemberStatus::Kicked),
            date: 1700000000,
            invite_link: None,
            via_join_request: None,
            via_chat_folder_invite_link: None,
        };
        roundtrip(&event);
    }

    // ── Payment events ────────────────────────────────────────────────────────

    #[test]
    fn test_pre_checkout_query_event_roundtrip() {
        let event = PreCheckoutQueryEvent {
            metadata: test_metadata(),
            pre_checkout_query_id: "pcq1".to_string(),
            from: User { id: 60, is_bot: false, first_name: "Eve".to_string(), last_name: None, username: None, language_code: None },
            currency: "USD".to_string(),
            total_amount: 1000,
            invoice_payload: "order_42".to_string(),
            shipping_option_id: None,
            order_info: None,
        };
        roundtrip(&event);
    }

    #[test]
    fn test_shipping_query_event_roundtrip() {
        use crate::chat::ShippingAddress;
        let event = ShippingQueryEvent {
            metadata: test_metadata(),
            shipping_query_id: "sq1".to_string(),
            from: User { id: 70, is_bot: false, first_name: "Frank".to_string(), last_name: None, username: None, language_code: None },
            invoice_payload: "payload_1".to_string(),
            shipping_address: ShippingAddress {
                country_code: "US".to_string(),
                state: "CA".to_string(),
                city: "San Francisco".to_string(),
                street_line1: "123 Main St".to_string(),
                street_line2: "".to_string(),
                post_code: "94102".to_string(),
            },
        };
        roundtrip(&event);
    }

    #[test]
    fn test_successful_payment_event_roundtrip() {
        use crate::chat::SuccessfulPayment;
        let event = SuccessfulPaymentEvent {
            metadata: test_metadata(),
            message: test_message(),
            payment: SuccessfulPayment {
                currency: "EUR".to_string(),
                total_amount: 500,
                invoice_payload: "inv_1".to_string(),
                telegram_payment_charge_id: "tg_charge_1".to_string(),
                provider_payment_charge_id: "provider_charge_1".to_string(),
                shipping_option_id: None,
                order_info: None,
            },
        };
        roundtrip(&event);
    }

    // ── Forum topic events ────────────────────────────────────────────────────

    #[test]
    fn test_forum_topic_created_event_roundtrip() {
        let event = ForumTopicCreatedEvent {
            metadata: test_metadata(),
            message: test_message(),
            forum_topic: ForumTopic { message_thread_id: 1, name: "Support".to_string(), icon_color: 0x6FB9F0, icon_custom_emoji_id: None },
        };
        roundtrip(&event);
    }

    #[test]
    fn test_forum_topic_closed_event_roundtrip() {
        let event = ForumTopicClosedEvent { metadata: test_metadata(), message: test_message() };
        roundtrip(&event);
    }
}
