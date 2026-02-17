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
