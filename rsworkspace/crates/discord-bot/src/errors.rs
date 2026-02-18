//! Discord-specific error handling for the bot.
//!
//! Converts serenity errors into structured, actionable `ErrorOutcome`s
//! and provides a `log_error` helper that logs at the correct level based
//! on whether the error is permanent, transient, or a rate-limit.

use std::time::Duration;

use discord_types::errors::{CommandErrorEvent, DiscordErrorCode};
use serenity::http::HttpError;
use tracing::{debug, error, warn};

/// Result of handling a Discord API error.
#[derive(Debug)]
pub enum ErrorOutcome {
    /// Retry the operation after this duration (rate limit / transient).
    Retry(Duration),
    /// Permanent failure; do not retry.
    Permanent(CommandErrorEvent),
    /// Non-permanent failure; log and continue.
    Transient(CommandErrorEvent),
}

/// Classify a serenity `Error` and return the appropriate `ErrorOutcome`.
pub fn classify(command_subject: &str, err: &serenity::Error) -> ErrorOutcome {
    match err {
        serenity::Error::Http(http_err) => classify_http(command_subject, http_err),
        _ => {
            let evt = make_event(
                command_subject,
                DiscordErrorCode::NetworkError,
                &err.to_string(),
                None,
                0,
                0,
            );
            debug!("Non-HTTP serenity error on '{}': {}", command_subject, err);
            ErrorOutcome::Transient(evt)
        }
    }
}

/// Log a serenity error at the appropriate level.
///
/// - Permanent errors → `error!`
/// - Rate-limited → `warn!`
/// - Transient errors → `warn!`
pub fn log_error(command_subject: &str, context: &str, err: &serenity::Error) {
    log_outcome(command_subject, context, classify(command_subject, err));
}

/// Log a pre-classified `ErrorOutcome` at the appropriate level.
pub fn log_outcome(command_subject: &str, context: &str, outcome: ErrorOutcome) {
    match outcome {
        ErrorOutcome::Permanent(evt) => {
            error!("{} [{:?}]: {}", context, evt.error_code, evt.message);
        }
        ErrorOutcome::Transient(evt) => {
            warn!("{} [{:?}]: {}", context, evt.error_code, evt.message);
        }
        ErrorOutcome::Retry(dur) => {
            warn!("{}: rate limited, retry after {:?}", command_subject, dur);
            let _ = context;
        }
    }
}

fn classify_http(command_subject: &str, http_err: &HttpError) -> ErrorOutcome {
    match http_err {
        HttpError::UnsuccessfulRequest(resp) => {
            let status = resp.status_code.as_u16();

            // HTTP 429 — rate limited
            if status == 429 {
                let retry_secs = 1u64; // conservative; real value is in the JSON body
                warn!(
                    "Rate limited on '{}': retry after {}s",
                    command_subject, retry_secs
                );
                return ErrorOutcome::Retry(Duration::from_secs(retry_secs));
            }

            let raw_code = resp.error.code as u32;
            let discord_code = DiscordErrorCode::from_raw(raw_code);
            let message = resp.error.message.clone();

            let evt = make_event(
                command_subject,
                discord_code.clone(),
                &message,
                None,
                raw_code,
                status,
            );

            if discord_code.is_permanent() {
                warn!(
                    "Permanent Discord error on '{}' (HTTP {} / code {}): {}",
                    command_subject, status, raw_code, message
                );
                ErrorOutcome::Permanent(evt)
            } else if discord_code.is_retryable() {
                warn!(
                    "Retryable Discord error on '{}' (HTTP {} / code {}): {}",
                    command_subject, status, raw_code, message
                );
                ErrorOutcome::Retry(Duration::from_secs(5))
            } else {
                debug!(
                    "Transient Discord error on '{}' (HTTP {} / code {}): {}",
                    command_subject, status, raw_code, message
                );
                ErrorOutcome::Transient(evt)
            }
        }

        // Network / request-level failures (not Discord API errors)
        _ => {
            let evt = make_event(
                command_subject,
                DiscordErrorCode::NetworkError,
                &http_err.to_string(),
                None,
                0,
                0,
            );
            debug!(
                "Network-level HTTP error on '{}': {}",
                command_subject, http_err
            );
            ErrorOutcome::Transient(evt)
        }
    }
}

fn make_event(
    command_subject: &str,
    code: DiscordErrorCode,
    message: &str,
    retry_after_secs: Option<u64>,
    raw_discord_code: u32,
    http_status: u16,
) -> CommandErrorEvent {
    let is_permanent = code.is_permanent();
    let category = code.category();
    CommandErrorEvent {
        command_subject: command_subject.to_string(),
        error_code: code,
        category,
        message: message.to_string(),
        raw_discord_code,
        http_status,
        retry_after_secs,
        is_permanent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use discord_types::errors::{DiscordErrorCode, ErrorCategory};

    // We can't easily construct serenity errors in unit tests without a live
    // HTTP client, so we test make_event() directly.

    #[test]
    fn test_make_event_permanent_permission() {
        let evt = make_event(
            "discord.prod.agent.message.send",
            DiscordErrorCode::MissingPermissions,
            "Missing Permissions",
            None,
            50013,
            403,
        );
        assert_eq!(evt.command_subject, "discord.prod.agent.message.send");
        assert_eq!(evt.error_code, DiscordErrorCode::MissingPermissions);
        assert_eq!(evt.category, ErrorCategory::PermissionDenied);
        assert_eq!(evt.raw_discord_code, 50013);
        assert_eq!(evt.http_status, 403);
        assert!(evt.is_permanent);
        assert!(evt.retry_after_secs.is_none());
    }

    #[test]
    fn test_make_event_rate_limited_with_retry_after() {
        let evt = make_event(
            "subj",
            DiscordErrorCode::RateLimited,
            "Rate limited",
            Some(5),
            0,
            429,
        );
        assert_eq!(evt.error_code, DiscordErrorCode::RateLimited);
        assert_eq!(evt.category, ErrorCategory::RateLimit);
        assert_eq!(evt.retry_after_secs, Some(5));
        assert!(!evt.is_permanent);
    }

    #[test]
    fn test_make_event_network_error() {
        let evt = make_event(
            "subj",
            DiscordErrorCode::NetworkError,
            "connection reset",
            None,
            0,
            0,
        );
        assert_eq!(evt.error_code, DiscordErrorCode::NetworkError);
        assert_eq!(evt.category, ErrorCategory::Network);
        assert!(!evt.is_permanent);
    }

    #[test]
    fn test_make_event_unknown_code() {
        let evt = make_event(
            "subj",
            DiscordErrorCode::Unknown,
            "something went wrong",
            None,
            99999,
            500,
        );
        assert_eq!(evt.error_code, DiscordErrorCode::Unknown);
        assert!(!evt.is_permanent);
    }

    #[test]
    fn test_make_event_api_overloaded_not_permanent() {
        let evt = make_event(
            "subj",
            DiscordErrorCode::ApiOverloaded,
            "API overloaded",
            None,
            130000,
            503,
        );
        assert_eq!(evt.error_code, DiscordErrorCode::ApiOverloaded);
        assert!(!evt.is_permanent);
    }

    #[test]
    fn test_make_event_not_found() {
        let evt = make_event(
            "subj",
            DiscordErrorCode::UnknownMessage,
            "Unknown Message",
            None,
            10008,
            404,
        );
        assert_eq!(evt.error_code, DiscordErrorCode::UnknownMessage);
        assert_eq!(evt.category, ErrorCategory::NotFound);
        assert!(evt.is_permanent);
    }
}
