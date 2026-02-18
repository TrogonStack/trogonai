//! Discord-specific error types for cross-crate use.
//!
//! Mirrors the structure in `telegram-types::errors` so that agents can handle
//! errors from both platforms with a consistent model.

use serde::{Deserialize, Serialize};

/// High-level category of a Discord API error.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    /// Rate limit hit — must wait before retrying.
    RateLimit,
    /// Target resource (channel, message, user …) not found.
    NotFound,
    /// Insufficient bot permissions for the requested action.
    PermissionDenied,
    /// Message cannot be modified (deleted, too old, not owned …).
    MessageUnmodifiable,
    /// Request payload too large.
    PayloadTooLarge,
    /// Malformed or semantically invalid input.
    InvalidInput,
    /// Network or I/O error (transient).
    Network,
    /// Unknown or uncategorised error.
    Unknown,
}

/// Discord-specific error code (subset relevant to bot operations).
///
/// Maps the most actionable Discord JSON error codes
/// (<https://discord.com/developers/docs/topics/opcodes-and-status-codes#json>)
/// to named variants; everything else falls through to [`DiscordErrorCode::Unknown`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiscordErrorCode {
    // ── Not found ─────────────────────────────────────────────────────────────
    /// 10003 — Unknown channel.
    UnknownChannel,
    /// 10004 — Unknown guild.
    UnknownGuild,
    /// 10008 — Unknown message (likely deleted).
    UnknownMessage,
    /// 10007 — Unknown member.
    UnknownMember,
    /// 10013 — Unknown user.
    UnknownUser,
    /// 10062 — Unknown interaction (token expired or already acknowledged).
    UnknownInteraction,

    // ── Permission errors ──────────────────────────────────────────────────────
    /// 50001 — Missing access.
    MissingAccess,
    /// 50013 — Missing permissions.
    MissingPermissions,
    /// 50007 — Cannot send messages to this user.
    CannotSendToUser,
    /// 50005 — Cannot edit a message authored by another user.
    CannotEditByOtherUser,

    // ── Rate limiting ──────────────────────────────────────────────────────────
    /// HTTP 429 — Global or per-route rate limit.
    RateLimited,
    /// 20016 — Action blocked by channel slowmode.
    SlowmodeRateLimit,
    /// 20028 — Channel write rate limit reached.
    ChannelWriteRateLimit,

    // ── Message / input errors ─────────────────────────────────────────────────
    /// 50006 — Cannot send an empty message.
    CannotSendEmptyMessage,
    /// 50035 — Invalid form body (validation failed).
    InvalidFormBody,
    /// 160005 / 50074 — Thread is locked or archived.
    ThreadLocked,
    /// 160002 — Cannot reply without permission to read message history.
    CannotReplyWithoutHistory,

    // ── Resource limits ────────────────────────────────────────────────────────
    /// 40005 / HTTP 413 — Request entity too large.
    RequestEntityTooLarge,
    /// 30003 — Maximum number of pins reached.
    MaxPinsReached,
    /// 30010 — Maximum number of reactions reached.
    MaxReactionsReached,

    // ── Auth ───────────────────────────────────────────────────────────────────
    /// 50014 / 40001 — Invalid or expired token.
    InvalidToken,

    // ── Server errors ──────────────────────────────────────────────────────────
    /// 130000 — API resource overloaded.
    ApiOverloaded,

    // ── Client errors ─────────────────────────────────────────────────────────
    /// Network or I/O error on the client side.
    NetworkError,

    // ── Catch-all ─────────────────────────────────────────────────────────────
    /// Any Discord JSON error code not listed above.
    Unknown,
}

impl DiscordErrorCode {
    /// Derive the code from a raw Discord JSON error code integer.
    pub fn from_raw(code: u32) -> Self {
        match code {
            10003 => Self::UnknownChannel,
            10004 => Self::UnknownGuild,
            10007 => Self::UnknownMember,
            10008 => Self::UnknownMessage,
            10013 => Self::UnknownUser,
            10062 => Self::UnknownInteraction,
            20016 => Self::SlowmodeRateLimit,
            20028 => Self::ChannelWriteRateLimit,
            30003 => Self::MaxPinsReached,
            30010 => Self::MaxReactionsReached,
            40001 => Self::InvalidToken,
            40005 => Self::RequestEntityTooLarge,
            50001 => Self::MissingAccess,
            50005 => Self::CannotEditByOtherUser,
            50006 => Self::CannotSendEmptyMessage,
            50007 => Self::CannotSendToUser,
            50013 => Self::MissingPermissions,
            50014 | 50025 | 50027 => Self::InvalidToken,
            50035 => Self::InvalidFormBody,
            50074 | 160005 => Self::ThreadLocked,
            160002 => Self::CannotReplyWithoutHistory,
            130000 => Self::ApiOverloaded,
            _ => Self::Unknown,
        }
    }

    /// The high-level category for this code.
    pub fn category(&self) -> ErrorCategory {
        match self {
            Self::UnknownChannel
            | Self::UnknownGuild
            | Self::UnknownMessage
            | Self::UnknownMember
            | Self::UnknownUser
            | Self::UnknownInteraction => ErrorCategory::NotFound,

            Self::MissingAccess
            | Self::MissingPermissions
            | Self::CannotSendToUser
            | Self::CannotEditByOtherUser => ErrorCategory::PermissionDenied,

            Self::RateLimited | Self::SlowmodeRateLimit | Self::ChannelWriteRateLimit => {
                ErrorCategory::RateLimit
            }

            Self::RequestEntityTooLarge => ErrorCategory::PayloadTooLarge,

            Self::CannotSendEmptyMessage
            | Self::InvalidFormBody
            | Self::ThreadLocked
            | Self::CannotReplyWithoutHistory
            | Self::MaxPinsReached
            | Self::MaxReactionsReached => ErrorCategory::InvalidInput,

            Self::NetworkError => ErrorCategory::Network,

            Self::InvalidToken | Self::ApiOverloaded | Self::Unknown => ErrorCategory::Unknown,
        }
    }

    /// True if the operation should **not** be retried (the error is permanent).
    pub fn is_permanent(&self) -> bool {
        matches!(
            self,
            Self::UnknownChannel
                | Self::UnknownGuild
                | Self::UnknownMessage
                | Self::UnknownMember
                | Self::UnknownUser
                | Self::MissingAccess
                | Self::MissingPermissions
                | Self::CannotSendToUser
                | Self::CannotEditByOtherUser
                | Self::InvalidToken
        )
    }

    /// True if retrying the operation after a delay is worthwhile.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited | Self::NetworkError | Self::ApiOverloaded
        )
    }
}

/// Error event published via NATS when a bot command fails.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandErrorEvent {
    /// NATS subject of the failed command.
    pub command_subject: String,
    /// Classified Discord error code.
    pub error_code: DiscordErrorCode,
    /// High-level error category.
    pub category: ErrorCategory,
    /// Human-readable error message from the Discord API.
    pub message: String,
    /// Raw Discord JSON error code (0 if not an API error).
    pub raw_discord_code: u32,
    /// HTTP status code (0 if not an HTTP error).
    pub http_status: u16,
    /// Seconds to wait before retrying (set for rate-limit errors).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
    /// True if the operation must not be retried.
    pub is_permanent: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ErrorCategory serde ───────────────────────────────────────────────────

    #[test]
    fn test_error_category_serde() {
        for (variant, expected) in [
            (ErrorCategory::RateLimit, "\"rate_limit\""),
            (ErrorCategory::NotFound, "\"not_found\""),
            (ErrorCategory::PermissionDenied, "\"permission_denied\""),
            (
                ErrorCategory::MessageUnmodifiable,
                "\"message_unmodifiable\"",
            ),
            (ErrorCategory::PayloadTooLarge, "\"payload_too_large\""),
            (ErrorCategory::InvalidInput, "\"invalid_input\""),
            (ErrorCategory::Network, "\"network\""),
            (ErrorCategory::Unknown, "\"unknown\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected, "unexpected JSON for {:?}", variant);
            let back: ErrorCategory = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant);
        }
    }

    // ── DiscordErrorCode::from_raw ────────────────────────────────────────────

    #[test]
    fn test_from_raw_known_codes() {
        assert_eq!(
            DiscordErrorCode::from_raw(10003),
            DiscordErrorCode::UnknownChannel
        );
        assert_eq!(
            DiscordErrorCode::from_raw(10008),
            DiscordErrorCode::UnknownMessage
        );
        assert_eq!(
            DiscordErrorCode::from_raw(50013),
            DiscordErrorCode::MissingPermissions
        );
        assert_eq!(
            DiscordErrorCode::from_raw(50001),
            DiscordErrorCode::MissingAccess
        );
        assert_eq!(
            DiscordErrorCode::from_raw(130000),
            DiscordErrorCode::ApiOverloaded
        );
    }

    #[test]
    fn test_from_raw_token_aliases() {
        assert_eq!(
            DiscordErrorCode::from_raw(40001),
            DiscordErrorCode::InvalidToken
        );
        assert_eq!(
            DiscordErrorCode::from_raw(50025),
            DiscordErrorCode::InvalidToken
        );
        assert_eq!(
            DiscordErrorCode::from_raw(50027),
            DiscordErrorCode::InvalidToken
        );
    }

    #[test]
    fn test_from_raw_thread_aliases() {
        assert_eq!(
            DiscordErrorCode::from_raw(160005),
            DiscordErrorCode::ThreadLocked
        );
        assert_eq!(
            DiscordErrorCode::from_raw(50074),
            DiscordErrorCode::ThreadLocked
        );
    }

    #[test]
    fn test_from_raw_unknown_falls_through() {
        assert_eq!(DiscordErrorCode::from_raw(99999), DiscordErrorCode::Unknown);
        assert_eq!(DiscordErrorCode::from_raw(0), DiscordErrorCode::Unknown);
    }

    // ── DiscordErrorCode::category ────────────────────────────────────────────

    #[test]
    fn test_category_not_found() {
        assert_eq!(
            DiscordErrorCode::UnknownChannel.category(),
            ErrorCategory::NotFound
        );
        assert_eq!(
            DiscordErrorCode::UnknownMessage.category(),
            ErrorCategory::NotFound
        );
        assert_eq!(
            DiscordErrorCode::UnknownInteraction.category(),
            ErrorCategory::NotFound
        );
    }

    #[test]
    fn test_category_permission_denied() {
        assert_eq!(
            DiscordErrorCode::MissingPermissions.category(),
            ErrorCategory::PermissionDenied
        );
        assert_eq!(
            DiscordErrorCode::MissingAccess.category(),
            ErrorCategory::PermissionDenied
        );
        assert_eq!(
            DiscordErrorCode::CannotSendToUser.category(),
            ErrorCategory::PermissionDenied
        );
    }

    #[test]
    fn test_category_rate_limit() {
        assert_eq!(
            DiscordErrorCode::RateLimited.category(),
            ErrorCategory::RateLimit
        );
        assert_eq!(
            DiscordErrorCode::SlowmodeRateLimit.category(),
            ErrorCategory::RateLimit
        );
    }

    #[test]
    fn test_category_invalid_input() {
        assert_eq!(
            DiscordErrorCode::CannotSendEmptyMessage.category(),
            ErrorCategory::InvalidInput
        );
        assert_eq!(
            DiscordErrorCode::ThreadLocked.category(),
            ErrorCategory::InvalidInput
        );
        assert_eq!(
            DiscordErrorCode::InvalidFormBody.category(),
            ErrorCategory::InvalidInput
        );
    }

    // ── DiscordErrorCode::is_permanent ────────────────────────────────────────

    #[test]
    fn test_is_permanent_true() {
        for code in [
            DiscordErrorCode::UnknownChannel,
            DiscordErrorCode::UnknownGuild,
            DiscordErrorCode::UnknownMessage,
            DiscordErrorCode::MissingAccess,
            DiscordErrorCode::MissingPermissions,
            DiscordErrorCode::CannotSendToUser,
            DiscordErrorCode::InvalidToken,
        ] {
            assert!(code.is_permanent(), "{:?} should be permanent", code);
        }
    }

    #[test]
    fn test_is_permanent_false() {
        for code in [
            DiscordErrorCode::RateLimited,
            DiscordErrorCode::NetworkError,
            DiscordErrorCode::ApiOverloaded,
            DiscordErrorCode::Unknown,
            DiscordErrorCode::SlowmodeRateLimit,
            DiscordErrorCode::ThreadLocked,
        ] {
            assert!(!code.is_permanent(), "{:?} should not be permanent", code);
        }
    }

    // ── DiscordErrorCode::is_retryable ────────────────────────────────────────

    #[test]
    fn test_is_retryable() {
        assert!(DiscordErrorCode::RateLimited.is_retryable());
        assert!(DiscordErrorCode::NetworkError.is_retryable());
        assert!(DiscordErrorCode::ApiOverloaded.is_retryable());

        assert!(!DiscordErrorCode::MissingPermissions.is_retryable());
        assert!(!DiscordErrorCode::UnknownMessage.is_retryable());
        assert!(!DiscordErrorCode::InvalidToken.is_retryable());
    }

    // ── DiscordErrorCode serde ────────────────────────────────────────────────

    #[test]
    fn test_error_code_serde_spot_check() {
        for (code, expected) in [
            (DiscordErrorCode::UnknownChannel, "\"unknown_channel\""),
            (
                DiscordErrorCode::MissingPermissions,
                "\"missing_permissions\"",
            ),
            (DiscordErrorCode::RateLimited, "\"rate_limited\""),
            (DiscordErrorCode::NetworkError, "\"network_error\""),
            (DiscordErrorCode::Unknown, "\"unknown\""),
        ] {
            let json = serde_json::to_string(&code).unwrap();
            assert_eq!(json, expected, "unexpected JSON for {:?}", code);
            let back: DiscordErrorCode = serde_json::from_str(&json).unwrap();
            assert_eq!(back, code);
        }
    }

    // ── CommandErrorEvent serde ───────────────────────────────────────────────

    #[test]
    fn test_command_error_event_roundtrip() {
        let evt = CommandErrorEvent {
            command_subject: "discord.prod.agent.message.send".to_string(),
            error_code: DiscordErrorCode::MissingPermissions,
            category: ErrorCategory::PermissionDenied,
            message: "Missing Permissions".to_string(),
            raw_discord_code: 50013,
            http_status: 403,
            retry_after_secs: None,
            is_permanent: true,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: CommandErrorEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.command_subject, evt.command_subject);
        assert_eq!(back.error_code, evt.error_code);
        assert_eq!(back.category, evt.category);
        assert_eq!(back.raw_discord_code, 50013);
        assert_eq!(back.http_status, 403);
        assert!(back.is_permanent);
        assert!(back.retry_after_secs.is_none());
    }

    #[test]
    fn test_command_error_event_retry_after_omitted_when_none() {
        let evt = CommandErrorEvent {
            command_subject: "subj".to_string(),
            error_code: DiscordErrorCode::Unknown,
            category: ErrorCategory::Unknown,
            message: "err".to_string(),
            raw_discord_code: 0,
            http_status: 500,
            retry_after_secs: None,
            is_permanent: false,
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(
            !json.contains("retry_after_secs"),
            "retry_after_secs must be omitted when None"
        );
    }
}
