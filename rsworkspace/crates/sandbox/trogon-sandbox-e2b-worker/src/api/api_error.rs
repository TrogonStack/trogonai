use reqwest::StatusCode;
use serde_json::Value;

use super::constants::MAX_ERROR_MESSAGE_CHARS;

/// Why a call to E2B failed, split by what a caller can do about it.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum ApiError {
    /// 401 or 403: the API key is missing, wrong, or not allowed to do this.
    #[error("E2B rejected the credentials: {0}")]
    Unauthorized(String),
    #[error("E2B resource not found: {0}")]
    NotFound(String),
    /// 409: the sandbox is in a state that refuses the call (for example already paused).
    #[error("E2B refused the call in the current state: {0}")]
    Conflict(String),
    /// 429: the account is over a rate or concurrency limit.
    #[error("E2B rate limit reached{}: {message}", retry_hint(*.retry_after_secs))]
    RateLimited {
        retry_after_secs: Option<u64>,
        message: String,
    },
    /// Any other 4xx: the request itself is wrong.
    #[error("E2B rejected the request (HTTP {status}): {message}")]
    Rejected { status: u16, message: String },
    /// 5xx: E2B failed, retrying later may help.
    #[error("E2B is unavailable (HTTP {status}): {message}")]
    Unavailable { status: u16, message: String },
    /// The request never got an HTTP answer, or the answer was not understood.
    #[error("E2B request failed: {0}")]
    Transport(String),
}

fn retry_hint(retry_after_secs: Option<u64>) -> String {
    retry_after_secs.map_or_else(String::new, |secs| format!(", retry after {secs}s"))
}

impl ApiError {
    pub(crate) fn from_status(status: StatusCode, retry_after: Option<u64>, body: &[u8]) -> Self {
        let message = error_message(body).unwrap_or_else(|| status.canonical_reason().unwrap_or("error").to_owned());
        match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Self::Unauthorized(message),
            StatusCode::NOT_FOUND => Self::NotFound(message),
            StatusCode::CONFLICT => Self::Conflict(message),
            StatusCode::TOO_MANY_REQUESTS => Self::RateLimited {
                retry_after_secs: retry_after,
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

    /// A transport failure that carries no request URL, since the URL can name a sandbox.
    pub(crate) fn transport(error: reqwest::Error) -> Self {
        Self::Transport(error.without_url().to_string())
    }
}

/// E2B error bodies are `{"code": 400, "message": "..."}`; Connect errors from envd are
/// `{"code": "not_found", "message": "..."}`.
pub(crate) fn error_message(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let message = value.get("message")?.as_str()?;
    Some(message.chars().take(MAX_ERROR_MESSAGE_CHARS).collect())
}
