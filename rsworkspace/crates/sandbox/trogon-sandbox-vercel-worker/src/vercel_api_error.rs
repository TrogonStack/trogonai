use reqwest::StatusCode;
use serde_json::Value;

use crate::constants::MESSAGE_LIMIT;

/// Why a call to Vercel failed, split by what a caller can do about it.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum VercelApiError {
    /// 401 or 403: the token is missing, wrong, or not allowed to do this.
    #[error("Vercel rejected the credentials: {0}")]
    Unauthorized(String),
    #[error("Vercel resource not found: {0}")]
    NotFound(String),
    /// 409: the sandbox is in a state that refuses the call.
    #[error("Vercel refused the call in the current state: {0}")]
    Conflict(String),
    /// 429: the account is over a rate or concurrency limit.
    #[error("Vercel rate limit reached{}: {message}", retry_hint(*.retry_after_secs))]
    RateLimited {
        retry_after_secs: Option<u64>,
        message: String,
    },
    /// Any other 4xx: the request itself is wrong.
    #[error("Vercel rejected the request (HTTP {status}): {message}")]
    Rejected { status: u16, message: String },
    /// 5xx: Vercel failed, retrying later may help.
    #[error("Vercel is unavailable (HTTP {status}): {message}")]
    Unavailable { status: u16, message: String },
    /// The request never got an HTTP answer, or the answer was not understood.
    #[error("Vercel request failed: {0}")]
    Transport(String),
}

fn retry_hint(retry_after_secs: Option<u64>) -> String {
    retry_after_secs.map_or_else(String::new, |secs| format!(", retry after {secs}s"))
}

impl VercelApiError {
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
        Self::Transport(format!("unexpected Vercel response: {error}"))
    }
}

/// Vercel error bodies are `{"error": {"code": "...", "message": "..."}}`.
fn error_message(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let message = value
        .get("error")
        .and_then(|error| error.get("message"))
        .or_else(|| value.get("message"))?
        .as_str()?;
    Some(message.chars().take(MESSAGE_LIMIT).collect())
}

#[cfg(test)]
mod tests;
