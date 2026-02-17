//! Telegram-specific error types for cross-crate use

use serde::{Deserialize, Serialize};

/// Classification of Telegram API errors for agents to act on
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    /// Rate limit hit - must wait before retrying
    RateLimit,
    /// Bot was blocked/kicked by user or group
    BotBlocked,
    /// Target chat or user not found
    NotFound,
    /// Insufficient bot permissions
    PermissionDenied,
    /// Message cannot be modified (too old, not owned, etc.)
    MessageUnmodifiable,
    /// Chat migrated to new ID
    ChatMigrated,
    /// Request payload too large
    PayloadTooLarge,
    /// Invalid input (bad file ID, bad URL, bad entities, etc.)
    InvalidInput,
    /// Network or I/O error (transient)
    Network,
    /// Webhook misconfiguration
    WebhookError,
    /// Unknown or uncategorized API error
    Unknown,
}

/// Telegram-specific error code
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TelegramErrorCode {
    // Rate limiting
    FloodControl,

    // Bot status
    BotBlocked,
    BotKicked,
    BotKickedFromSupergroup,
    BotKickedFromChannel,
    InvalidToken,
    CantInitiateConversation,
    CantTalkWithBots,

    // Message operations
    MessageNotModified,
    MessageIdInvalid,
    MessageToForwardNotFound,
    MessageToDeleteNotFound,
    MessageToCopyNotFound,
    MessageTextIsEmpty,
    MessageCantBeEdited,
    MessageCantBeDeleted,
    MessageToEditNotFound,
    MessageToReplyNotFound,
    MessageIdentifierNotSpecified,
    MessageIsTooLong,
    EditedMessageIsTooLong,
    TooMuchMessages,

    // Chat/user
    ChatNotFound,
    UserNotFound,
    ChatDescriptionIsNotModified,
    GroupDeactivated,
    UserDeactivated,

    // Migrations
    MigrateToChatId,

    // Permissions
    NotEnoughRightsToPinMessage,
    NotEnoughRightsToManagePins,
    NotEnoughRightsToChangeChatPermissions,
    NotEnoughRightsToRestrict,
    NotEnoughRightsToPostMessages,
    MethodNotAvailableInPrivateChats,
    CantDemoteChatCreator,
    CantRestrictSelf,

    // Files
    WrongFileId,
    WrongFileIdOrUrl,
    FailedToGetUrlContent,
    FileIdInvalid,
    RequestEntityTooLarge,
    PhotoAsInputFileRequired,

    // Parse / entities
    CantParseEntities,
    CantParseUrl,
    ButtonUrlInvalid,
    ButtonDataInvalid,

    // Callbacks / inline
    InvalidQueryId,
    TooMuchInlineQueryResults,

    // Polls
    PollHasAlreadyClosed,
    PollMustHaveMoreOptions,
    PollCantHaveMoreOptions,
    PollOptionsMustBeNonEmpty,
    PollQuestionMustBeNonEmpty,
    MessageIsNotAPoll,

    // Webhooks
    WebhookRequireHttps,
    BadWebhookPort,
    UnknownHost,

    // Updates
    CantGetUpdates,
    TerminatedByOtherGetUpdates,

    // Stickers
    InvalidStickersSet,
    StickerSetNameOccupied,

    // Network
    NetworkError,
    IoError,

    // Unknown
    Unknown,
}

impl TelegramErrorCode {
    /// Returns the error category for this code
    pub fn category(&self) -> ErrorCategory {
        match self {
            Self::FloodControl => ErrorCategory::RateLimit,

            Self::BotBlocked
            | Self::BotKicked
            | Self::BotKickedFromSupergroup
            | Self::BotKickedFromChannel
            | Self::CantInitiateConversation
            | Self::CantTalkWithBots => ErrorCategory::BotBlocked,

            Self::ChatNotFound
            | Self::UserNotFound
            | Self::MessageToForwardNotFound
            | Self::MessageToDeleteNotFound
            | Self::MessageToCopyNotFound
            | Self::MessageToEditNotFound
            | Self::MessageToReplyNotFound
            | Self::GroupDeactivated
            | Self::UserDeactivated => ErrorCategory::NotFound,

            Self::NotEnoughRightsToPinMessage
            | Self::NotEnoughRightsToManagePins
            | Self::NotEnoughRightsToChangeChatPermissions
            | Self::NotEnoughRightsToRestrict
            | Self::NotEnoughRightsToPostMessages
            | Self::MethodNotAvailableInPrivateChats
            | Self::CantDemoteChatCreator
            | Self::CantRestrictSelf => ErrorCategory::PermissionDenied,

            Self::MessageNotModified
            | Self::MessageCantBeEdited
            | Self::MessageCantBeDeleted
            | Self::ChatDescriptionIsNotModified => ErrorCategory::MessageUnmodifiable,

            Self::MigrateToChatId => ErrorCategory::ChatMigrated,

            Self::RequestEntityTooLarge
            | Self::MessageIsTooLong
            | Self::EditedMessageIsTooLong
            | Self::TooMuchMessages
            | Self::TooMuchInlineQueryResults => ErrorCategory::PayloadTooLarge,

            Self::WrongFileId
            | Self::WrongFileIdOrUrl
            | Self::FailedToGetUrlContent
            | Self::FileIdInvalid
            | Self::PhotoAsInputFileRequired
            | Self::CantParseEntities
            | Self::CantParseUrl
            | Self::ButtonUrlInvalid
            | Self::ButtonDataInvalid
            | Self::MessageIdInvalid
            | Self::MessageIdentifierNotSpecified
            | Self::MessageTextIsEmpty
            | Self::InvalidToken
            | Self::InvalidQueryId => ErrorCategory::InvalidInput,

            Self::NetworkError | Self::IoError => ErrorCategory::Network,

            Self::WebhookRequireHttps
            | Self::BadWebhookPort
            | Self::UnknownHost => ErrorCategory::WebhookError,

            _ => ErrorCategory::Unknown,
        }
    }

    /// Returns true if the error is permanent (no retry should be attempted)
    pub fn is_permanent(&self) -> bool {
        matches!(
            self,
            Self::BotBlocked
                | Self::BotKicked
                | Self::BotKickedFromSupergroup
                | Self::BotKickedFromChannel
                | Self::InvalidToken
                | Self::ChatNotFound
                | Self::UserDeactivated
                | Self::GroupDeactivated
                | Self::CantDemoteChatCreator
                | Self::MessageCantBeEdited
                | Self::MessageCantBeDeleted
                | Self::MessageNotModified
                | Self::ChatDescriptionIsNotModified
        )
    }

    /// Returns true if retry is worthwhile for this error
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::FloodControl | Self::NetworkError | Self::IoError | Self::CantGetUpdates
        )
    }
}

/// Error event published via NATS when a bot operation fails
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandErrorEvent {
    /// The NATS subject of the failed command
    pub command_subject: String,
    /// Telegram error code
    pub error_code: TelegramErrorCode,
    /// Error category
    pub category: ErrorCategory,
    /// Human-readable error message
    pub message: String,
    /// Seconds to wait before retrying (only for RateLimit)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
    /// New chat ID if the chat was migrated
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migrated_to_chat_id: Option<i64>,
    /// True if the operation should not be retried
    pub is_permanent: bool,
}
