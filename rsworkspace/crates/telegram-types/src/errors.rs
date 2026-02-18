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

            Self::WebhookRequireHttps | Self::BadWebhookPort | Self::UnknownHost => {
                ErrorCategory::WebhookError
            }

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

#[cfg(test)]
mod tests {
    use super::*;

    // ── ErrorCategory serde ───────────────────────────────────────────────────

    #[test]
    fn test_error_category_serde() {
        for (variant, expected) in [
            (ErrorCategory::RateLimit, "\"rate_limit\""),
            (ErrorCategory::BotBlocked, "\"bot_blocked\""),
            (ErrorCategory::NotFound, "\"not_found\""),
            (ErrorCategory::PermissionDenied, "\"permission_denied\""),
            (
                ErrorCategory::MessageUnmodifiable,
                "\"message_unmodifiable\"",
            ),
            (ErrorCategory::ChatMigrated, "\"chat_migrated\""),
            (ErrorCategory::PayloadTooLarge, "\"payload_too_large\""),
            (ErrorCategory::InvalidInput, "\"invalid_input\""),
            (ErrorCategory::Network, "\"network\""),
            (ErrorCategory::WebhookError, "\"webhook_error\""),
            (ErrorCategory::Unknown, "\"unknown\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected, "Unexpected JSON for {:?}", variant);
            let back: ErrorCategory = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant);
        }
    }

    // ── TelegramErrorCode serde ───────────────────────────────────────────────

    #[test]
    fn test_telegram_error_code_serde_spot_check() {
        // Verify key codes serialize to expected snake_case JSON
        for (code, expected) in [
            (TelegramErrorCode::BotBlocked, "\"bot_blocked\""),
            (TelegramErrorCode::ChatNotFound, "\"chat_not_found\""),
            (
                TelegramErrorCode::MessageNotModified,
                "\"message_not_modified\"",
            ),
            (TelegramErrorCode::FloodControl, "\"flood_control\""),
            (TelegramErrorCode::NetworkError, "\"network_error\""),
            (TelegramErrorCode::Unknown, "\"unknown\""),
        ] {
            let json = serde_json::to_string(&code).unwrap();
            assert_eq!(json, expected, "Unexpected JSON for {:?}", code);
            let back: TelegramErrorCode = serde_json::from_str(&json).unwrap();
            assert_eq!(back, code);
        }
    }

    // ── TelegramErrorCode logic methods ───────────────────────────────────────

    #[test]
    fn test_flood_control_is_rate_limit_category() {
        assert_eq!(
            TelegramErrorCode::FloodControl.category(),
            ErrorCategory::RateLimit
        );
    }

    #[test]
    fn test_migrate_to_chat_id_is_chat_migrated_category() {
        assert_eq!(
            TelegramErrorCode::MigrateToChatId.category(),
            ErrorCategory::ChatMigrated
        );
    }

    #[test]
    fn test_request_entity_too_large_is_payload_category() {
        assert_eq!(
            TelegramErrorCode::RequestEntityTooLarge.category(),
            ErrorCategory::PayloadTooLarge
        );
    }

    #[test]
    fn test_poll_errors_are_unknown_category() {
        // Poll errors fall through to Unknown category
        assert_eq!(
            TelegramErrorCode::PollHasAlreadyClosed.category(),
            ErrorCategory::Unknown
        );
        assert_eq!(
            TelegramErrorCode::PollMustHaveMoreOptions.category(),
            ErrorCategory::Unknown
        );
    }

    #[test]
    fn test_is_permanent_comprehensive() {
        let permanent = [
            TelegramErrorCode::BotBlocked,
            TelegramErrorCode::BotKicked,
            TelegramErrorCode::BotKickedFromSupergroup,
            TelegramErrorCode::BotKickedFromChannel,
            TelegramErrorCode::InvalidToken,
            TelegramErrorCode::ChatNotFound,
            TelegramErrorCode::UserDeactivated,
            TelegramErrorCode::GroupDeactivated,
            TelegramErrorCode::CantDemoteChatCreator,
            TelegramErrorCode::MessageCantBeEdited,
            TelegramErrorCode::MessageCantBeDeleted,
            TelegramErrorCode::MessageNotModified,
            TelegramErrorCode::ChatDescriptionIsNotModified,
        ];
        for code in &permanent {
            assert!(code.is_permanent(), "{:?} should be permanent", code);
        }
    }

    #[test]
    fn test_is_retryable_comprehensive() {
        let retryable = [
            TelegramErrorCode::FloodControl,
            TelegramErrorCode::NetworkError,
            TelegramErrorCode::IoError,
            TelegramErrorCode::CantGetUpdates,
        ];
        for code in &retryable {
            assert!(code.is_retryable(), "{:?} should be retryable", code);
        }

        // Permanent errors are NOT retryable
        assert!(!TelegramErrorCode::BotBlocked.is_retryable());
        assert!(!TelegramErrorCode::ChatNotFound.is_retryable());
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
    /// Chat ID associated with the failed command (when available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<i64>,
    /// Session ID associated with the failed command (from command metadata headers)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}
