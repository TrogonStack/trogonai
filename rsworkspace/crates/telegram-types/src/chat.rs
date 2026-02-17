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

/// Chat member status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatMemberStatus {
    Creator,
    Administrator,
    Member,
    Restricted,
    Left,
    Kicked,
}

/// Chat member information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMember {
    pub user: User,
    pub status: ChatMemberStatus,
    /// Date when restrictions will be lifted (for restricted/kicked users)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until_date: Option<i64>,
    /// True if the user's presence in the chat is hidden
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_anonymous: Option<bool>,
    /// Custom title for administrator
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_title: Option<String>,
}

/// Chat invite link information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatInviteLink {
    pub invite_link: String,
    pub creator: User,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub creates_join_request: bool,
    pub is_primary: bool,
    pub is_revoked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expire_date: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_limit: Option<i32>,
}

/// Chat permissions for all members
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatPermissions {
    /// True if users can send text messages, contacts, locations, and venues
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_send_messages: Option<bool>,
    /// True if users can send photos, videos, video notes, and voice notes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_send_media_messages: Option<bool>,
    /// True if users can send polls
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_send_polls: Option<bool>,
    /// True if users can send animations, games, stickers, and use inline bots
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_send_other_messages: Option<bool>,
    /// True if users can add web page previews
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_add_web_page_previews: Option<bool>,
    /// True if users can change the chat title, photo, and other settings
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_change_info: Option<bool>,
    /// True if users can invite new users to the chat
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_invite_users: Option<bool>,
    /// True if users can pin messages
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_pin_messages: Option<bool>,
    /// True if users can create forum topics
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_manage_topics: Option<bool>,
}

/// Administrator rights in a chat
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatAdministratorRights {
    /// True if the user's presence in the chat is hidden
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_anonymous: Option<bool>,
    /// True if the administrator can access the chat event log
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_manage_chat: Option<bool>,
    /// True if the administrator can delete messages of other users
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_delete_messages: Option<bool>,
    /// True if the administrator can manage video chats
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_manage_video_chats: Option<bool>,
    /// True if the administrator can restrict, ban, or unban chat members
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_restrict_members: Option<bool>,
    /// True if the administrator can add new administrators
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_promote_members: Option<bool>,
    /// True if the user can change the chat title, photo, and other settings
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_change_info: Option<bool>,
    /// True if the user can invite new users to the chat
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_invite_users: Option<bool>,
    /// True if the administrator can pin messages (supergroups only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_pin_messages: Option<bool>,
    /// True if the user can create, rename, close, and reopen forum topics (supergroups only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_manage_topics: Option<bool>,
    /// True if the administrator can post messages in the channel (channels only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_post_messages: Option<bool>,
    /// True if the administrator can edit messages of other users (channels only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_edit_messages: Option<bool>,
    /// True if the user can post stories to the chat (channels only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_post_stories: Option<bool>,
    /// True if the user can edit stories posted to the chat (channels only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_edit_stories: Option<bool>,
    /// True if the user can delete stories posted to the chat (channels only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_delete_stories: Option<bool>,
}

/// Text entity type (mention, hashtag, url, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageEntityType {
    Mention,           // @username
    Hashtag,           // #hashtag
    Cashtag,           // $USD
    BotCommand,        // /start
    Url,               // https://telegram.org
    Email,             // do-not-reply@telegram.org
    PhoneNumber,       // +1-212-555-0123
    Bold,              // bold text
    Italic,            // italic text
    Underline,         // underlined text
    Strikethrough,     // strikethrough text
    Spoiler,           // spoiler message
    Code,              // monowidth string
    Pre,               // monowidth block
    TextLink,          // clickable text URLs
    TextMention,       // users without usernames
    CustomEmoji,       // inline custom emoji stickers
}

/// Message entity (mention, hashtag, URL, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEntity {
    /// Type of the entity
    #[serde(rename = "type")]
    pub entity_type: MessageEntityType,
    /// Offset in UTF-16 code units to the start of the entity
    pub offset: i32,
    /// Length of the entity in UTF-16 code units
    pub length: i32,
    /// For "text_link" only, URL that will be opened after user taps on the text
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// For "text_mention" only, the mentioned user
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<User>,
    /// For "pre" only, the programming language of the entity text
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// For "custom_emoji" only, unique identifier of the custom emoji
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_emoji_id: Option<String>,
}

/// Bot command for bot menu
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotCommand {
    /// Command text (1-32 characters, only lowercase letters, digits and underscores)
    pub command: String,
    /// Command description (1-256 characters)
    pub description: String,
}

/// Scope for bot commands
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BotCommandScope {
    /// Commands for all users
    Default,
    /// Commands for all private chats
    AllPrivateChats,
    /// Commands for all group chats
    AllGroupChats,
    /// Commands for all chat administrators
    AllChatAdministrators,
    /// Commands for specific chat
    Chat { chat_id: i64 },
    /// Commands for chat administrators in specific chat
    ChatAdministrators { chat_id: i64 },
    /// Commands for specific user in specific chat
    ChatMember { chat_id: i64, user_id: i64 },
}

/// Price portion for invoice
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabeledPrice {
    /// Portion label
    pub label: String,
    /// Price in the smallest units of the currency (integer, not float)
    /// For example, for $1.45 pass amount = 145
    pub amount: i64,
}

/// Invoice information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invoice {
    /// Product name
    pub title: String,
    /// Product description
    pub description: String,
    /// Unique bot deep-linking parameter for invoice
    pub start_parameter: String,
    /// Three-letter ISO 4217 currency code
    pub currency: String,
    /// Total price in the smallest units of the currency
    pub total_amount: i64,
}

/// Shipping address
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShippingAddress {
    /// ISO 3166-1 alpha-2 country code
    pub country_code: String,
    /// State, if applicable
    pub state: String,
    /// City
    pub city: String,
    /// First line for the address
    pub street_line1: String,
    /// Second line for the address
    pub street_line2: String,
    /// Address post code
    pub post_code: String,
}

/// Order info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderInfo {
    /// User name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// User's phone number
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone_number: Option<String>,
    /// User email
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// User shipping address
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shipping_address: Option<ShippingAddress>,
}

/// Shipping option
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShippingOption {
    /// Shipping option identifier
    pub id: String,
    /// Option title
    pub title: String,
    /// List of price portions
    pub prices: Vec<LabeledPrice>,
}

/// Sticker set info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StickerSet {
    /// Short name of the sticker set
    pub name: String,
    /// Sticker set title
    pub title: String,
    /// Kind: regular, mask, or custom_emoji
    pub kind: String,
    /// List of stickers in the set (file_ids)
    pub sticker_file_ids: Vec<String>,
    /// Thumbnail file_id if set
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_file_id: Option<String>,
}

/// Mask point (face part) for mask stickers
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MaskPoint {
    Forehead,
    Eyes,
    Mouth,
    Chin,
}

/// Mask position for mask stickers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaskPosition {
    pub point: MaskPoint,
    pub x_shift: f64,
    pub y_shift: f64,
    pub scale: f64,
}

/// Input sticker for creating/adding to sticker sets
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputSticker {
    /// file_id of existing sticker or URL
    pub sticker: String,
    /// Format: "static", "animated", or "video"
    pub format: String,
    /// List of 1-20 emoji associated with the sticker
    pub emoji_list: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask_position: Option<MaskPosition>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub keywords: Vec<String>,
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

    // ── ChatType ──────────────────────────────────────────────────────────────

    #[test]
    fn test_chat_type_serde() {
        for (variant, expected) in [
            (ChatType::Private, "\"private\""),
            (ChatType::Group, "\"group\""),
            (ChatType::Supergroup, "\"supergroup\""),
            (ChatType::Channel, "\"channel\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected);
            let back: ChatType = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant);
        }
    }

    // ── Chat ──────────────────────────────────────────────────────────────────

    #[test]
    fn test_chat_roundtrip_private() {
        let chat = Chat {
            id: 12345,
            chat_type: ChatType::Private,
            title: None,
            username: Some("alice".to_string()),
        };
        roundtrip(&chat);
    }

    #[test]
    fn test_chat_roundtrip_group() {
        let chat = Chat {
            id: -1001234567890,
            chat_type: ChatType::Group,
            title: Some("My Group".to_string()),
            username: None,
        };
        roundtrip(&chat);
    }

    #[test]
    fn test_chat_type_field_is_renamed_to_type() {
        let chat = Chat {
            id: 1,
            chat_type: ChatType::Private,
            title: None,
            username: None,
        };
        let json = serde_json::to_string(&chat).unwrap();
        assert!(json.contains("\"type\":\"private\""));
        assert!(!json.contains("\"chat_type\""));
    }

    // ── User ──────────────────────────────────────────────────────────────────

    #[test]
    fn test_user_roundtrip_minimal() {
        let user = User {
            id: 999,
            is_bot: false,
            first_name: "Alice".to_string(),
            last_name: None,
            username: None,
            language_code: None,
        };
        roundtrip(&user);
    }

    #[test]
    fn test_user_roundtrip_full() {
        let user = User {
            id: 42,
            is_bot: true,
            first_name: "TestBot".to_string(),
            last_name: Some("Bot".to_string()),
            username: Some("testbot".to_string()),
            language_code: Some("en".to_string()),
        };
        roundtrip(&user);
    }

    // ── FileInfo ──────────────────────────────────────────────────────────────

    #[test]
    fn test_file_info_roundtrip() {
        let fi = FileInfo {
            file_id: "AgACAgIAAxk".to_string(),
            file_unique_id: "AQADuJ0xG".to_string(),
            file_size: Some(102400),
            file_name: Some("document.pdf".to_string()),
            mime_type: Some("application/pdf".to_string()),
        };
        roundtrip(&fi);
    }

    #[test]
    fn test_file_info_optional_fields_serialize_as_null() {
        // FileInfo fields are Option but not marked skip_serializing_if,
        // so None serializes as null (not omitted)
        let fi = FileInfo {
            file_id: "id".to_string(),
            file_unique_id: "uid".to_string(),
            file_size: None,
            file_name: None,
            mime_type: None,
        };
        let json = serde_json::to_string(&fi).unwrap();
        assert!(json.contains("\"file_size\":null"));
        assert!(json.contains("\"file_name\":null"));
        assert!(json.contains("\"mime_type\":null"));
    }

    // ── PhotoSize ─────────────────────────────────────────────────────────────

    #[test]
    fn test_photo_size_roundtrip() {
        let ps = PhotoSize {
            file_id: "file123".to_string(),
            file_unique_id: "unique123".to_string(),
            width: 1920,
            height: 1080,
            file_size: Some(512000),
        };
        roundtrip(&ps);
    }

    // ── InlineKeyboardMarkup ──────────────────────────────────────────────────

    #[test]
    fn test_inline_keyboard_new() {
        let btn = InlineKeyboardButton {
            text: "Click me".to_string(),
            callback_data: Some("action:1".to_string()),
            url: None,
        };
        let kb = InlineKeyboardMarkup::new(vec![vec![btn]]);
        assert_eq!(kb.inline_keyboard.len(), 1);
        assert_eq!(kb.inline_keyboard[0].len(), 1);
        assert_eq!(kb.inline_keyboard[0][0].text, "Click me");
    }

    #[test]
    fn test_inline_keyboard_button_url_omitted_when_none() {
        let btn = InlineKeyboardButton {
            text: "Btn".to_string(),
            callback_data: Some("cb".to_string()),
            url: None,
        };
        let json = serde_json::to_string(&btn).unwrap();
        assert!(!json.contains("\"url\""));
        assert!(json.contains("\"callback_data\""));
    }

    #[test]
    fn test_inline_keyboard_roundtrip() {
        let kb = InlineKeyboardMarkup::new(vec![
            vec![
                InlineKeyboardButton { text: "A".to_string(), callback_data: Some("a".to_string()), url: None },
                InlineKeyboardButton { text: "B".to_string(), callback_data: None, url: Some("https://t.me".to_string()) },
            ],
            vec![
                InlineKeyboardButton { text: "C".to_string(), callback_data: Some("c".to_string()), url: None },
            ],
        ]);
        roundtrip(&kb);
    }

    // ── ChatMemberStatus ──────────────────────────────────────────────────────

    #[test]
    fn test_chat_member_status_serde() {
        for (variant, expected) in [
            (ChatMemberStatus::Creator, "\"creator\""),
            (ChatMemberStatus::Administrator, "\"administrator\""),
            (ChatMemberStatus::Member, "\"member\""),
            (ChatMemberStatus::Restricted, "\"restricted\""),
            (ChatMemberStatus::Left, "\"left\""),
            (ChatMemberStatus::Kicked, "\"kicked\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected);
            let back: ChatMemberStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant);
        }
    }

    // ── MessageEntityType ─────────────────────────────────────────────────────

    #[test]
    fn test_message_entity_type_serde() {
        for (variant, expected) in [
            (MessageEntityType::Mention, "\"mention\""),
            (MessageEntityType::BotCommand, "\"bot_command\""),
            (MessageEntityType::Url, "\"url\""),
            (MessageEntityType::Bold, "\"bold\""),
            (MessageEntityType::Code, "\"code\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected);
            let back: MessageEntityType = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant);
        }
    }

    // ── BotCommandScope (tagged enum) ─────────────────────────────────────────

    #[test]
    fn test_bot_command_scope_serde() {
        let scopes = [
            BotCommandScope::Default,
            BotCommandScope::AllPrivateChats,
            BotCommandScope::AllGroupChats,
            BotCommandScope::Chat { chat_id: 12345 },
            BotCommandScope::ChatMember { chat_id: 10, user_id: 20 },
        ];
        for scope in &scopes {
            let json = serde_json::to_string(scope).unwrap();
            let back: BotCommandScope = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&back).unwrap();
            assert_eq!(json, json2);
        }
    }

    #[test]
    fn test_bot_command_scope_default_has_type_field() {
        let scope = BotCommandScope::Default;
        let json = serde_json::to_string(&scope).unwrap();
        assert_eq!(json, "{\"type\":\"default\"}");
    }

    // ── MaskPoint ─────────────────────────────────────────────────────────────

    #[test]
    fn test_mask_point_serde() {
        for (variant, expected) in [
            (MaskPoint::Forehead, "\"forehead\""),
            (MaskPoint::Eyes, "\"eyes\""),
            (MaskPoint::Mouth, "\"mouth\""),
            (MaskPoint::Chin, "\"chin\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected);
            let back: MaskPoint = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant);
        }
    }
}

/// Successful payment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessfulPayment {
    /// Three-letter ISO 4217 currency code
    pub currency: String,
    /// Total price in the smallest units of the currency
    pub total_amount: i64,
    /// Bot specified invoice payload
    pub invoice_payload: String,
    /// Telegram payment identifier
    pub telegram_payment_charge_id: String,
    /// Provider payment identifier
    pub provider_payment_charge_id: String,
    /// Identifier of the shipping option chosen by the user
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shipping_option_id: Option<String>,
    /// Order info provided by the user
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_info: Option<OrderInfo>,
}
