//! Telegram-specific error handling
//!
//! Converts teloxide errors into structured, actionable types and
//! implements retry strategies for transient failures.

use std::time::Duration;

use teloxide::types::ChatId;
use teloxide::RequestError;
use tracing::{debug, warn};

use telegram_types::errors::{CommandErrorEvent, TelegramErrorCode};

/// Result of handling an API error
#[derive(Debug)]
pub enum ErrorOutcome {
    /// Retry after this duration
    Retry(Duration),
    /// Chat migrated to new ID; caller should update and retry
    Migrated(ChatId),
    /// Permanent failure; do not retry
    Permanent(CommandErrorEvent),
    /// Non-permanent failure; log and continue
    Transient(CommandErrorEvent),
}

/// Classify a `RequestError` and decide what to do next.
pub fn classify(command_subject: &str, err: &RequestError) -> ErrorOutcome {
    match err {
        // ── Flood control ────────────────────────────────────────────────────
        RequestError::RetryAfter(secs) => {
            let wait = Duration::from_secs(secs.duration().as_secs().max(1));
            warn!(
                "Flood control on '{}': retry after {:?}",
                command_subject, wait
            );
            ErrorOutcome::Retry(wait)
        }

        // ── Chat migration ───────────────────────────────────────────────────
        RequestError::MigrateToChatId(new_id) => {
            warn!(
                "Chat migrated on '{}': new_id={}",
                command_subject, new_id.0
            );
            ErrorOutcome::Migrated(*new_id)
        }

        // ── Network / I/O ────────────────────────────────────────────────────
        RequestError::Network(_) => {
            let evt = make_event(
                command_subject,
                TelegramErrorCode::NetworkError,
                &err.to_string(),
                None,
                None,
            );
            debug!("Network error on '{}': {}", command_subject, err);
            ErrorOutcome::Transient(evt)
        }

        RequestError::Io(_) => {
            let evt = make_event(
                command_subject,
                TelegramErrorCode::IoError,
                &err.to_string(),
                None,
                None,
            );
            debug!("I/O error on '{}': {}", command_subject, err);
            ErrorOutcome::Transient(evt)
        }

        // ── Invalid JSON ─────────────────────────────────────────────────────
        RequestError::InvalidJson { raw, .. } => {
            let msg = format!("Invalid JSON response: {}", raw);
            let evt = make_event(
                command_subject,
                TelegramErrorCode::Unknown,
                &msg,
                None,
                None,
            );
            warn!("Invalid JSON on '{}': {}", command_subject, msg);
            ErrorOutcome::Transient(evt)
        }

        // ── Telegram API errors ──────────────────────────────────────────────
        RequestError::Api(api_err) => classify_api(command_subject, api_err),
    }
}

fn classify_api(command_subject: &str, api_err: &teloxide::ApiError) -> ErrorOutcome {
    use teloxide::ApiError;

    let (code, message) = match api_err {
        // Bot status ──────────────────────────────────────────────────────────
        ApiError::BotBlocked => (
            TelegramErrorCode::BotBlocked,
            "Bot was blocked by the user".into(),
        ),
        ApiError::BotKicked => (
            TelegramErrorCode::BotKicked,
            "Bot was kicked from the group".into(),
        ),
        ApiError::BotKickedFromSupergroup => (
            TelegramErrorCode::BotKickedFromSupergroup,
            "Bot was kicked from the supergroup".into(),
        ),
        ApiError::BotKickedFromChannel => (
            TelegramErrorCode::BotKickedFromChannel,
            "Bot was kicked from the channel".into(),
        ),
        ApiError::InvalidToken => (
            TelegramErrorCode::InvalidToken,
            "Bot token is invalid".into(),
        ),
        ApiError::CantInitiateConversation => (
            TelegramErrorCode::CantInitiateConversation,
            "Can't initiate conversation with the user".into(),
        ),
        ApiError::CantTalkWithBots => (
            TelegramErrorCode::CantTalkWithBots,
            "Can't send messages to bots".into(),
        ),

        // Message operations ──────────────────────────────────────────────────
        ApiError::MessageNotModified => (
            TelegramErrorCode::MessageNotModified,
            "Message content and reply markup are exactly the same".into(),
        ),
        ApiError::MessageIdInvalid => (
            TelegramErrorCode::MessageIdInvalid,
            "Message ID is invalid".into(),
        ),
        ApiError::MessageToForwardNotFound => (
            TelegramErrorCode::MessageToForwardNotFound,
            "Message to forward not found".into(),
        ),
        ApiError::MessageToDeleteNotFound => (
            TelegramErrorCode::MessageToDeleteNotFound,
            "Message to delete not found".into(),
        ),
        ApiError::MessageToCopyNotFound => (
            TelegramErrorCode::MessageToCopyNotFound,
            "Message to copy not found".into(),
        ),
        ApiError::MessageTextIsEmpty => (
            TelegramErrorCode::MessageTextIsEmpty,
            "Message text must not be empty".into(),
        ),
        ApiError::MessageCantBeEdited => (
            TelegramErrorCode::MessageCantBeEdited,
            "Message can't be edited".into(),
        ),
        ApiError::MessageCantBeDeleted => (
            TelegramErrorCode::MessageCantBeDeleted,
            "Message can't be deleted".into(),
        ),
        ApiError::MessageToEditNotFound => (
            TelegramErrorCode::MessageToEditNotFound,
            "Message to edit not found".into(),
        ),
        ApiError::MessageToReplyNotFound => (
            TelegramErrorCode::MessageToReplyNotFound,
            "Message to reply to not found".into(),
        ),
        ApiError::MessageIdentifierNotSpecified => (
            TelegramErrorCode::MessageIdentifierNotSpecified,
            "Message identifier not specified".into(),
        ),
        ApiError::MessageIsTooLong => (
            TelegramErrorCode::MessageIsTooLong,
            "Message is too long (max 4096 characters)".into(),
        ),
        ApiError::EditedMessageIsTooLong => (
            TelegramErrorCode::EditedMessageIsTooLong,
            "Edited message is too long".into(),
        ),
        ApiError::TooMuchMessages => (
            TelegramErrorCode::TooMuchMessages,
            "Too many messages sent to this chat".into(),
        ),

        // Chat / user ─────────────────────────────────────────────────────────
        ApiError::ChatNotFound => (TelegramErrorCode::ChatNotFound, "Chat not found".into()),
        ApiError::UserNotFound => (TelegramErrorCode::UserNotFound, "User not found".into()),
        ApiError::ChatDescriptionIsNotModified => (
            TelegramErrorCode::ChatDescriptionIsNotModified,
            "Chat description is not modified".into(),
        ),
        ApiError::GroupDeactivated => (
            TelegramErrorCode::GroupDeactivated,
            "Group is deactivated".into(),
        ),
        ApiError::UserDeactivated => (
            TelegramErrorCode::UserDeactivated,
            "User is deactivated".into(),
        ),

        // Permissions ─────────────────────────────────────────────────────────
        ApiError::NotEnoughRightsToPinMessage => (
            TelegramErrorCode::NotEnoughRightsToPinMessage,
            "Not enough rights to pin a message".into(),
        ),
        ApiError::NotEnoughRightsToManagePins => (
            TelegramErrorCode::NotEnoughRightsToManagePins,
            "Not enough rights to manage pins".into(),
        ),
        ApiError::NotEnoughRightsToChangeChatPermissions => (
            TelegramErrorCode::NotEnoughRightsToChangeChatPermissions,
            "Not enough rights to change chat permissions".into(),
        ),
        ApiError::NotEnoughRightsToRestrict => (
            TelegramErrorCode::NotEnoughRightsToRestrict,
            "Not enough rights to restrict/promote chat members".into(),
        ),
        ApiError::NotEnoughRightsToPostMessages => (
            TelegramErrorCode::NotEnoughRightsToPostMessages,
            "Not enough rights to post messages".into(),
        ),
        ApiError::MethodNotAvailableInPrivateChats => (
            TelegramErrorCode::MethodNotAvailableInPrivateChats,
            "This method is not available in private chats".into(),
        ),
        ApiError::CantDemoteChatCreator => (
            TelegramErrorCode::CantDemoteChatCreator,
            "Can't demote chat creator".into(),
        ),
        ApiError::CantRestrictSelf => (
            TelegramErrorCode::CantRestrictSelf,
            "Can't restrict self".into(),
        ),

        // Files ───────────────────────────────────────────────────────────────
        ApiError::WrongFileId => (TelegramErrorCode::WrongFileId, "Wrong file ID".into()),
        ApiError::WrongFileIdOrUrl => (
            TelegramErrorCode::WrongFileIdOrUrl,
            "Wrong file ID or URL".into(),
        ),
        ApiError::FailedToGetUrlContent => (
            TelegramErrorCode::FailedToGetUrlContent,
            "Failed to get content from URL".into(),
        ),
        ApiError::FileIdInvalid => (
            TelegramErrorCode::FileIdInvalid,
            "File ID is invalid".into(),
        ),
        ApiError::RequestEntityTooLarge => (
            TelegramErrorCode::RequestEntityTooLarge,
            "Request entity too large (file too big)".into(),
        ),
        ApiError::PhotoAsInputFileRequired => (
            TelegramErrorCode::PhotoAsInputFileRequired,
            "Photo must be uploaded as input file".into(),
        ),

        // Parse / entities ────────────────────────────────────────────────────
        ApiError::CantParseEntities(details) => (
            TelegramErrorCode::CantParseEntities,
            format!("Can't parse entities: {}", details),
        ),
        ApiError::CantParseUrl => (TelegramErrorCode::CantParseUrl, "Can't parse URL".into()),
        ApiError::ButtonUrlInvalid => (
            TelegramErrorCode::ButtonUrlInvalid,
            "Button URL is invalid".into(),
        ),
        ApiError::ButtonDataInvalid => (
            TelegramErrorCode::ButtonDataInvalid,
            "Button data is invalid or too long (max 64 bytes)".into(),
        ),
        ApiError::TextButtonsAreUnallowed => (
            TelegramErrorCode::ButtonDataInvalid,
            "Text buttons are not allowed when inline keyboard is used".into(),
        ),

        // Callbacks / inline ──────────────────────────────────────────────────
        ApiError::InvalidQueryId => (
            TelegramErrorCode::InvalidQueryId,
            "Query ID is invalid or expired (answer within 10s)".into(),
        ),
        ApiError::TooMuchInlineQueryResults => (
            TelegramErrorCode::TooMuchInlineQueryResults,
            "Too many inline query results (max 50)".into(),
        ),

        // Polls ───────────────────────────────────────────────────────────────
        ApiError::PollHasAlreadyClosed => (
            TelegramErrorCode::PollHasAlreadyClosed,
            "Poll has already been closed".into(),
        ),
        ApiError::PollMustHaveMoreOptions => (
            TelegramErrorCode::PollMustHaveMoreOptions,
            "Poll must have at least 2 options".into(),
        ),
        ApiError::PollCantHaveMoreOptions => (
            TelegramErrorCode::PollCantHaveMoreOptions,
            "Poll can't have more than 10 options".into(),
        ),
        ApiError::PollOptionsMustBeNonEmpty => (
            TelegramErrorCode::PollOptionsMustBeNonEmpty,
            "Poll options must be non-empty".into(),
        ),
        ApiError::PollQuestionMustBeNonEmpty => (
            TelegramErrorCode::PollQuestionMustBeNonEmpty,
            "Poll question must be non-empty".into(),
        ),
        ApiError::MessageWithPollNotFound => (
            TelegramErrorCode::MessageIdInvalid,
            "Message with poll not found".into(),
        ),
        ApiError::MessageIsNotAPoll => (
            TelegramErrorCode::MessageIdInvalid,
            "Message is not a poll".into(),
        ),

        // Webhooks ────────────────────────────────────────────────────────────
        ApiError::WebhookRequireHttps => (
            TelegramErrorCode::WebhookRequireHttps,
            "Webhook requires HTTPS".into(),
        ),
        ApiError::BadWebhookPort => (
            TelegramErrorCode::BadWebhookPort,
            "Bad webhook port (must be 443, 80, 88 or 8443)".into(),
        ),
        ApiError::UnknownHost => (
            TelegramErrorCode::UnknownHost,
            "Unknown host in webhook URL".into(),
        ),

        // Updates ─────────────────────────────────────────────────────────────
        ApiError::CantGetUpdates => (
            TelegramErrorCode::CantGetUpdates,
            "Can't get updates (use either long polling or webhooks)".into(),
        ),
        ApiError::TerminatedByOtherGetUpdates => (
            TelegramErrorCode::TerminatedByOtherGetUpdates,
            "Terminated by other getUpdates request".into(),
        ),

        // Stickers ────────────────────────────────────────────────────────────
        ApiError::InvalidStickersSet => (
            TelegramErrorCode::InvalidStickersSet,
            "Invalid stickers set".into(),
        ),
        ApiError::StickerSetNameOccupied => (
            TelegramErrorCode::StickerSetNameOccupied,
            "Sticker set name is already occupied".into(),
        ),

        // Catch-all ───────────────────────────────────────────────────────────
        ApiError::Unknown(raw) => (
            TelegramErrorCode::Unknown,
            format!("Unknown Telegram API error: {}", raw),
        ),

        _ => (
            TelegramErrorCode::Unknown,
            format!("Unhandled Telegram API error: {}", api_err),
        ),
    };

    let retry_after = if code == TelegramErrorCode::FloodControl {
        Some(30u64)
    } else {
        None
    };

    let evt = make_event(command_subject, code.clone(), &message, retry_after, None);

    if code.is_permanent() {
        warn!(
            "Permanent Telegram error on '{}' [{:?}]: {}",
            command_subject, evt.category, message
        );
        ErrorOutcome::Permanent(evt)
    } else {
        debug!(
            "Transient Telegram error on '{}' [{:?}]: {}",
            command_subject, evt.category, message
        );
        ErrorOutcome::Transient(evt)
    }
}

fn make_event(
    command_subject: &str,
    code: TelegramErrorCode,
    message: &str,
    retry_after_secs: Option<u64>,
    migrated_to_chat_id: Option<i64>,
) -> CommandErrorEvent {
    let is_permanent = code.is_permanent();
    let category = code.category();
    CommandErrorEvent {
        command_subject: command_subject.to_string(),
        error_code: code,
        category,
        message: message.to_string(),
        retry_after_secs,
        migrated_to_chat_id,
        is_permanent,
        chat_id: None,
        session_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use telegram_types::errors::{ErrorCategory, TelegramErrorCode};
    use teloxide::{ApiError, RequestError};

    // ── TelegramErrorCode::is_permanent() ─────────────────────────────────────

    #[test]
    fn test_is_permanent_true_for_bot_blocked() {
        assert!(TelegramErrorCode::BotBlocked.is_permanent());
        assert!(TelegramErrorCode::BotKicked.is_permanent());
        assert!(TelegramErrorCode::BotKickedFromSupergroup.is_permanent());
        assert!(TelegramErrorCode::BotKickedFromChannel.is_permanent());
        assert!(TelegramErrorCode::InvalidToken.is_permanent());
        assert!(TelegramErrorCode::ChatNotFound.is_permanent());
        assert!(TelegramErrorCode::UserDeactivated.is_permanent());
        assert!(TelegramErrorCode::GroupDeactivated.is_permanent());
    }

    #[test]
    fn test_is_permanent_true_for_message_errors() {
        assert!(TelegramErrorCode::MessageNotModified.is_permanent());
        assert!(TelegramErrorCode::MessageCantBeEdited.is_permanent());
        assert!(TelegramErrorCode::MessageCantBeDeleted.is_permanent());
        assert!(TelegramErrorCode::ChatDescriptionIsNotModified.is_permanent());
        assert!(TelegramErrorCode::CantDemoteChatCreator.is_permanent());
    }

    #[test]
    fn test_is_permanent_false_for_transient_errors() {
        assert!(!TelegramErrorCode::NetworkError.is_permanent());
        assert!(!TelegramErrorCode::IoError.is_permanent());
        assert!(!TelegramErrorCode::FloodControl.is_permanent());
        assert!(!TelegramErrorCode::PollHasAlreadyClosed.is_permanent());
        assert!(!TelegramErrorCode::Unknown.is_permanent());
    }

    // ── TelegramErrorCode::is_retryable() ─────────────────────────────────────

    #[test]
    fn test_is_retryable() {
        assert!(TelegramErrorCode::FloodControl.is_retryable());
        assert!(TelegramErrorCode::NetworkError.is_retryable());
        assert!(TelegramErrorCode::IoError.is_retryable());
        assert!(TelegramErrorCode::CantGetUpdates.is_retryable());

        assert!(!TelegramErrorCode::BotBlocked.is_retryable());
        assert!(!TelegramErrorCode::ChatNotFound.is_retryable());
        assert!(!TelegramErrorCode::MessageNotModified.is_retryable());
    }

    // ── TelegramErrorCode::category() ─────────────────────────────────────────

    #[test]
    fn test_category_bot_blocked() {
        assert_eq!(
            TelegramErrorCode::BotBlocked.category(),
            ErrorCategory::BotBlocked
        );
        assert_eq!(
            TelegramErrorCode::BotKicked.category(),
            ErrorCategory::BotBlocked
        );
        assert_eq!(
            TelegramErrorCode::CantInitiateConversation.category(),
            ErrorCategory::BotBlocked
        );
    }

    #[test]
    fn test_category_not_found() {
        assert_eq!(
            TelegramErrorCode::ChatNotFound.category(),
            ErrorCategory::NotFound
        );
        assert_eq!(
            TelegramErrorCode::UserNotFound.category(),
            ErrorCategory::NotFound
        );
        assert_eq!(
            TelegramErrorCode::GroupDeactivated.category(),
            ErrorCategory::NotFound
        );
    }

    #[test]
    fn test_category_permission_denied() {
        assert_eq!(
            TelegramErrorCode::NotEnoughRightsToPinMessage.category(),
            ErrorCategory::PermissionDenied
        );
        assert_eq!(
            TelegramErrorCode::NotEnoughRightsToPostMessages.category(),
            ErrorCategory::PermissionDenied
        );
        assert_eq!(
            TelegramErrorCode::CantRestrictSelf.category(),
            ErrorCategory::PermissionDenied
        );
    }

    #[test]
    fn test_category_message_unmodifiable() {
        assert_eq!(
            TelegramErrorCode::MessageNotModified.category(),
            ErrorCategory::MessageUnmodifiable
        );
        assert_eq!(
            TelegramErrorCode::MessageCantBeEdited.category(),
            ErrorCategory::MessageUnmodifiable
        );
        assert_eq!(
            TelegramErrorCode::MessageCantBeDeleted.category(),
            ErrorCategory::MessageUnmodifiable
        );
    }

    #[test]
    fn test_category_network() {
        assert_eq!(
            TelegramErrorCode::NetworkError.category(),
            ErrorCategory::Network
        );
        assert_eq!(
            TelegramErrorCode::IoError.category(),
            ErrorCategory::Network
        );
    }

    #[test]
    fn test_category_webhook_error() {
        assert_eq!(
            TelegramErrorCode::WebhookRequireHttps.category(),
            ErrorCategory::WebhookError
        );
        assert_eq!(
            TelegramErrorCode::BadWebhookPort.category(),
            ErrorCategory::WebhookError
        );
        assert_eq!(
            TelegramErrorCode::UnknownHost.category(),
            ErrorCategory::WebhookError
        );
    }

    // ── classify() with RequestError::Api ─────────────────────────────────────

    #[test]
    fn test_classify_bot_blocked_is_permanent() {
        let err = RequestError::Api(ApiError::BotBlocked);
        let outcome = classify("test.subject", &err);
        match outcome {
            ErrorOutcome::Permanent(evt) => {
                assert_eq!(evt.command_subject, "test.subject");
                assert_eq!(evt.error_code, TelegramErrorCode::BotBlocked);
                assert!(evt.is_permanent);
            }
            other => panic!("Expected Permanent, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_chat_not_found_is_permanent() {
        let err = RequestError::Api(ApiError::ChatNotFound);
        let outcome = classify("send.message", &err);
        match outcome {
            ErrorOutcome::Permanent(evt) => {
                assert_eq!(evt.error_code, TelegramErrorCode::ChatNotFound);
                assert!(evt.is_permanent);
                assert_eq!(evt.category, ErrorCategory::NotFound);
            }
            other => panic!("Expected Permanent, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_message_not_modified_is_permanent() {
        let err = RequestError::Api(ApiError::MessageNotModified);
        let outcome = classify("edit.message", &err);
        match outcome {
            ErrorOutcome::Permanent(evt) => {
                assert_eq!(evt.error_code, TelegramErrorCode::MessageNotModified);
                assert!(evt.is_permanent);
                assert_eq!(evt.category, ErrorCategory::MessageUnmodifiable);
            }
            other => panic!("Expected Permanent, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_poll_closed_is_transient() {
        let err = RequestError::Api(ApiError::PollHasAlreadyClosed);
        let outcome = classify("stop.poll", &err);
        match outcome {
            ErrorOutcome::Transient(evt) => {
                assert_eq!(evt.error_code, TelegramErrorCode::PollHasAlreadyClosed);
                assert!(!evt.is_permanent);
            }
            other => panic!("Expected Transient, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_retry_after() {
        use teloxide::types::Seconds;
        let err = RequestError::RetryAfter(Seconds::from_seconds(42));
        let outcome = classify("send.message", &err);
        match outcome {
            ErrorOutcome::Retry(duration) => {
                assert_eq!(duration.as_secs(), 42);
            }
            other => panic!("Expected Retry, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_migrate_to_chat_id() {
        let new_id = ChatId(9876);
        let err = RequestError::MigrateToChatId(new_id);
        let outcome = classify("send.message", &err);
        match outcome {
            ErrorOutcome::Migrated(id) => {
                assert_eq!(id, new_id);
            }
            other => panic!("Expected Migrated, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_io_error_is_transient() {
        let io_err = std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "connection reset",
        ));
        let err = RequestError::Io(io_err);
        let outcome = classify("send.message", &err);
        match outcome {
            ErrorOutcome::Transient(evt) => {
                assert_eq!(evt.error_code, TelegramErrorCode::IoError);
                assert!(!evt.is_permanent);
                assert_eq!(evt.category, ErrorCategory::Network);
            }
            other => panic!("Expected Transient, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_unknown_api_error_is_transient() {
        let err = RequestError::Api(ApiError::Unknown("some unknown error".to_string()));
        let outcome = classify("some.subject", &err);
        match outcome {
            ErrorOutcome::Transient(evt) => {
                assert_eq!(evt.error_code, TelegramErrorCode::Unknown);
                assert!(!evt.is_permanent);
            }
            other => panic!("Expected Transient, got {:?}", other),
        }
    }

    // ── classify() - various API errors ───────────────────────────────────────

    #[test]
    fn test_classify_wrong_file_id_is_transient() {
        let err = RequestError::Api(ApiError::WrongFileId);
        let outcome = classify("send.document", &err);
        match outcome {
            ErrorOutcome::Transient(evt) => {
                assert_eq!(evt.error_code, TelegramErrorCode::WrongFileId);
                assert_eq!(evt.category, ErrorCategory::InvalidInput);
            }
            other => panic!("Expected Transient, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_webhook_error_is_transient() {
        let err = RequestError::Api(ApiError::WebhookRequireHttps);
        let outcome = classify("set.webhook", &err);
        match outcome {
            ErrorOutcome::Transient(evt) => {
                assert_eq!(evt.error_code, TelegramErrorCode::WebhookRequireHttps);
                assert_eq!(evt.category, ErrorCategory::WebhookError);
            }
            other => panic!("Expected Transient, got {:?}", other),
        }
    }

    // ── CommandErrorEvent fields ───────────────────────────────────────────────

    #[test]
    fn test_command_error_event_has_correct_subject() {
        let subject = "telegram.prod.bot.message.send";
        let err = RequestError::Api(ApiError::BotBlocked);
        let outcome = classify(subject, &err);
        match outcome {
            ErrorOutcome::Permanent(evt) => {
                assert_eq!(evt.command_subject, subject);
            }
            other => panic!("Expected Permanent, got {:?}", other),
        }
    }

    #[test]
    fn test_command_error_event_serde_roundtrip() {
        use telegram_types::errors::CommandErrorEvent;
        let evt = CommandErrorEvent {
            command_subject: "test.subject".to_string(),
            error_code: TelegramErrorCode::ChatNotFound,
            category: ErrorCategory::NotFound,
            message: "Chat not found".to_string(),
            retry_after_secs: None,
            migrated_to_chat_id: None,
            is_permanent: true,
        };

        let json = serde_json::to_string(&evt).expect("serialize");
        let decoded: CommandErrorEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.command_subject, evt.command_subject);
        assert_eq!(decoded.error_code, evt.error_code);
        assert_eq!(decoded.category, evt.category);
        assert_eq!(decoded.message, evt.message);
        assert_eq!(decoded.is_permanent, evt.is_permanent);
        assert!(decoded.retry_after_secs.is_none());
        assert!(decoded.migrated_to_chat_id.is_none());
    }
}
