//! Telegram-specific error handling
//!
//! Converts teloxide errors into structured, actionable types and
//! implements retry strategies for transient failures.

use std::time::Duration;

use teloxide::RequestError;
use teloxide::types::ChatId;
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

fn classify_api(
    command_subject: &str,
    api_err: &teloxide::ApiError,
) -> ErrorOutcome {
    use teloxide::ApiError;

    let (code, message) = match api_err {
        // Bot status ──────────────────────────────────────────────────────────
        ApiError::BotBlocked => (TelegramErrorCode::BotBlocked, "Bot was blocked by the user".into()),
        ApiError::BotKicked => (TelegramErrorCode::BotKicked, "Bot was kicked from the group".into()),
        ApiError::BotKickedFromSupergroup => (TelegramErrorCode::BotKickedFromSupergroup, "Bot was kicked from the supergroup".into()),
        ApiError::BotKickedFromChannel => (TelegramErrorCode::BotKickedFromChannel, "Bot was kicked from the channel".into()),
        ApiError::InvalidToken => (TelegramErrorCode::InvalidToken, "Bot token is invalid".into()),
        ApiError::CantInitiateConversation => (TelegramErrorCode::CantInitiateConversation, "Can't initiate conversation with the user".into()),
        ApiError::CantTalkWithBots => (TelegramErrorCode::CantTalkWithBots, "Can't send messages to bots".into()),

        // Message operations ──────────────────────────────────────────────────
        ApiError::MessageNotModified => (TelegramErrorCode::MessageNotModified, "Message content and reply markup are exactly the same".into()),
        ApiError::MessageIdInvalid => (TelegramErrorCode::MessageIdInvalid, "Message ID is invalid".into()),
        ApiError::MessageToForwardNotFound => (TelegramErrorCode::MessageToForwardNotFound, "Message to forward not found".into()),
        ApiError::MessageToDeleteNotFound => (TelegramErrorCode::MessageToDeleteNotFound, "Message to delete not found".into()),
        ApiError::MessageToCopyNotFound => (TelegramErrorCode::MessageToCopyNotFound, "Message to copy not found".into()),
        ApiError::MessageTextIsEmpty => (TelegramErrorCode::MessageTextIsEmpty, "Message text must not be empty".into()),
        ApiError::MessageCantBeEdited => (TelegramErrorCode::MessageCantBeEdited, "Message can't be edited".into()),
        ApiError::MessageCantBeDeleted => (TelegramErrorCode::MessageCantBeDeleted, "Message can't be deleted".into()),
        ApiError::MessageToEditNotFound => (TelegramErrorCode::MessageToEditNotFound, "Message to edit not found".into()),
        ApiError::MessageToReplyNotFound => (TelegramErrorCode::MessageToReplyNotFound, "Message to reply to not found".into()),
        ApiError::MessageIdentifierNotSpecified => (TelegramErrorCode::MessageIdentifierNotSpecified, "Message identifier not specified".into()),
        ApiError::MessageIsTooLong => (TelegramErrorCode::MessageIsTooLong, "Message is too long (max 4096 characters)".into()),
        ApiError::EditedMessageIsTooLong => (TelegramErrorCode::EditedMessageIsTooLong, "Edited message is too long".into()),
        ApiError::TooMuchMessages => (TelegramErrorCode::TooMuchMessages, "Too many messages sent to this chat".into()),

        // Chat / user ─────────────────────────────────────────────────────────
        ApiError::ChatNotFound => (TelegramErrorCode::ChatNotFound, "Chat not found".into()),
        ApiError::UserNotFound => (TelegramErrorCode::UserNotFound, "User not found".into()),
        ApiError::ChatDescriptionIsNotModified => (TelegramErrorCode::ChatDescriptionIsNotModified, "Chat description is not modified".into()),
        ApiError::GroupDeactivated => (TelegramErrorCode::GroupDeactivated, "Group is deactivated".into()),
        ApiError::UserDeactivated => (TelegramErrorCode::UserDeactivated, "User is deactivated".into()),

        // Permissions ─────────────────────────────────────────────────────────
        ApiError::NotEnoughRightsToPinMessage => (TelegramErrorCode::NotEnoughRightsToPinMessage, "Not enough rights to pin a message".into()),
        ApiError::NotEnoughRightsToManagePins => (TelegramErrorCode::NotEnoughRightsToManagePins, "Not enough rights to manage pins".into()),
        ApiError::NotEnoughRightsToChangeChatPermissions => (TelegramErrorCode::NotEnoughRightsToChangeChatPermissions, "Not enough rights to change chat permissions".into()),
        ApiError::NotEnoughRightsToRestrict => (TelegramErrorCode::NotEnoughRightsToRestrict, "Not enough rights to restrict/promote chat members".into()),
        ApiError::NotEnoughRightsToPostMessages => (TelegramErrorCode::NotEnoughRightsToPostMessages, "Not enough rights to post messages".into()),
        ApiError::MethodNotAvailableInPrivateChats => (TelegramErrorCode::MethodNotAvailableInPrivateChats, "This method is not available in private chats".into()),
        ApiError::CantDemoteChatCreator => (TelegramErrorCode::CantDemoteChatCreator, "Can't demote chat creator".into()),
        ApiError::CantRestrictSelf => (TelegramErrorCode::CantRestrictSelf, "Can't restrict self".into()),

        // Files ───────────────────────────────────────────────────────────────
        ApiError::WrongFileId => (TelegramErrorCode::WrongFileId, "Wrong file ID".into()),
        ApiError::WrongFileIdOrUrl => (TelegramErrorCode::WrongFileIdOrUrl, "Wrong file ID or URL".into()),
        ApiError::FailedToGetUrlContent => (TelegramErrorCode::FailedToGetUrlContent, "Failed to get content from URL".into()),
        ApiError::FileIdInvalid => (TelegramErrorCode::FileIdInvalid, "File ID is invalid".into()),
        ApiError::RequestEntityTooLarge => (TelegramErrorCode::RequestEntityTooLarge, "Request entity too large (file too big)".into()),
        ApiError::PhotoAsInputFileRequired => (TelegramErrorCode::PhotoAsInputFileRequired, "Photo must be uploaded as input file".into()),

        // Parse / entities ────────────────────────────────────────────────────
        ApiError::CantParseEntities(details) => (TelegramErrorCode::CantParseEntities, format!("Can't parse entities: {}", details)),
        ApiError::CantParseUrl => (TelegramErrorCode::CantParseUrl, "Can't parse URL".into()),
        ApiError::ButtonUrlInvalid => (TelegramErrorCode::ButtonUrlInvalid, "Button URL is invalid".into()),
        ApiError::ButtonDataInvalid => (TelegramErrorCode::ButtonDataInvalid, "Button data is invalid or too long (max 64 bytes)".into()),
        ApiError::TextButtonsAreUnallowed => (TelegramErrorCode::ButtonDataInvalid, "Text buttons are not allowed when inline keyboard is used".into()),

        // Callbacks / inline ──────────────────────────────────────────────────
        ApiError::InvalidQueryId => (TelegramErrorCode::InvalidQueryId, "Query ID is invalid or expired (answer within 10s)".into()),
        ApiError::TooMuchInlineQueryResults => (TelegramErrorCode::TooMuchInlineQueryResults, "Too many inline query results (max 50)".into()),

        // Polls ───────────────────────────────────────────────────────────────
        ApiError::PollHasAlreadyClosed => (TelegramErrorCode::PollHasAlreadyClosed, "Poll has already been closed".into()),
        ApiError::PollMustHaveMoreOptions => (TelegramErrorCode::PollMustHaveMoreOptions, "Poll must have at least 2 options".into()),
        ApiError::PollCantHaveMoreOptions => (TelegramErrorCode::PollCantHaveMoreOptions, "Poll can't have more than 10 options".into()),
        ApiError::PollOptionsMustBeNonEmpty => (TelegramErrorCode::PollOptionsMustBeNonEmpty, "Poll options must be non-empty".into()),
        ApiError::PollQuestionMustBeNonEmpty => (TelegramErrorCode::PollQuestionMustBeNonEmpty, "Poll question must be non-empty".into()),
        ApiError::MessageWithPollNotFound => (TelegramErrorCode::MessageIdInvalid, "Message with poll not found".into()),
        ApiError::MessageIsNotAPoll => (TelegramErrorCode::MessageIdInvalid, "Message is not a poll".into()),

        // Webhooks ────────────────────────────────────────────────────────────
        ApiError::WebhookRequireHttps => (TelegramErrorCode::WebhookRequireHttps, "Webhook requires HTTPS".into()),
        ApiError::BadWebhookPort => (TelegramErrorCode::BadWebhookPort, "Bad webhook port (must be 443, 80, 88 or 8443)".into()),
        ApiError::UnknownHost => (TelegramErrorCode::UnknownHost, "Unknown host in webhook URL".into()),

        // Updates ─────────────────────────────────────────────────────────────
        ApiError::CantGetUpdates => (TelegramErrorCode::CantGetUpdates, "Can't get updates (use either long polling or webhooks)".into()),
        ApiError::TerminatedByOtherGetUpdates => (TelegramErrorCode::TerminatedByOtherGetUpdates, "Terminated by other getUpdates request".into()),

        // Stickers ────────────────────────────────────────────────────────────
        ApiError::InvalidStickersSet => (TelegramErrorCode::InvalidStickersSet, "Invalid stickers set".into()),
        ApiError::StickerSetNameOccupied => (TelegramErrorCode::StickerSetNameOccupied, "Sticker set name is already occupied".into()),

        // Catch-all ───────────────────────────────────────────────────────────
        ApiError::Unknown(raw) => (TelegramErrorCode::Unknown, format!("Unknown Telegram API error: {}", raw)),

        _ => (TelegramErrorCode::Unknown, format!("Unhandled Telegram API error: {}", api_err)),
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
    }
}
