use reqwest::StatusCode;
use serde::Deserialize;

use crate::constants::MESSAGE_LIMIT;

/// Why a call to Daytona failed, split by what a caller can do about it.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum DaytonaApiError {
    /// 401 or 403: the key is missing, wrong, or not allowed to do this.
    #[error("Daytona rejected the credentials: {0}")]
    Unauthorized(String),
    #[error("Daytona resource not found: {0}")]
    NotFound(String),
    /// 409: the sandbox is in a state that refuses the call.
    #[error("Daytona refused the call in the current state: {0}")]
    Conflict(String),
    /// 429: the account is over a rate or concurrency limit.
    #[error("Daytona rate limit reached{}: {message}", retry_hint(*.retry_after_secs))]
    RateLimited {
        retry_after_secs: Option<u64>,
        message: String,
    },
    /// Any other 4xx: the request itself is wrong.
    #[error("Daytona rejected the request (HTTP {status}): {message}")]
    Rejected { status: u16, message: String },
    /// 5xx: Daytona failed, retrying later may help.
    #[error("Daytona is unavailable (HTTP {status}): {message}")]
    Unavailable { status: u16, message: String },
    /// The request never got an HTTP answer, or the answer was not understood.
    #[error("Daytona request failed: {0}")]
    Transport(String),
}

/// Daytona error bodies are `{"statusCode": 404, "message": "...", ...}`.
#[derive(Deserialize)]
struct ErrorBody {
    message: Option<String>,
}

fn retry_hint(retry_after_secs: Option<u64>) -> String {
    retry_after_secs.map_or_else(String::new, |secs| format!(", retry after {secs}s"))
}

impl DaytonaApiError {
    pub(crate) fn from_status(status: StatusCode, retry_after_secs: Option<u64>, body: &[u8]) -> Self {
        let message = error_message(body).unwrap_or_else(|| status.canonical_reason().unwrap_or("error").to_owned());
        match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Self::Unauthorized(message),
            StatusCode::NOT_FOUND => Self::NotFound(message),
            StatusCode::CONFLICT => Self::Conflict(message),
            StatusCode::TOO_MANY_REQUESTS => Self::RateLimited {
                retry_after_secs,
                message,
            },
            status if status.is_server_error() => Self::Unavailable {
                status: status.as_u16(),
                message,
            },
            status => Self::Rejected {
                status: status.as_u16(),
                message,
            },
        }
    }

    pub(crate) fn transport(error: reqwest::Error) -> Self {
        Self::Transport(error.without_url().to_string())
    }

    pub(crate) fn unexpected(error: serde_json::Error) -> Self {
        Self::Transport(format!("unexpected Daytona response: {error}"))
    }
}

fn error_message(body: &[u8]) -> Option<String> {
    let parsed: ErrorBody = serde_json::from_slice(body).ok()?;
    Some(parsed.message?.chars().take(MESSAGE_LIMIT).collect())
}

#[cfg(test)]
mod tests;
