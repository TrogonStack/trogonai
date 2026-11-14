//! HTTP failures of the browser transport.

use serde_json::Value;

use crate::{
    js_text::{parse_json, strip_invisible, truncate_utf16},
    sse_utf8_decoder::SseUtf8Decoder,
};

const BODY_LIMIT: usize = 4096;
const REASON_LIMIT: usize = 128;
const PERMANENT_STATUSES: [u16; 3] = [401, 403, 404];

/// A failed HTTP request of the browser transport: what was attempted, the status code and, when the error body
/// named one, a short reason.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{context}: HTTP {code}")]
pub struct SseHttpError {
    context: String,
    code: u16,
    reason: Option<String>,
}

impl SseHttpError {
    /// An error for `context` (for example `SSE connect failed`) that got status `code`.
    pub fn new(context: impl Into<String>, code: u16, reason: Option<String>) -> Self {
        Self {
            context: context.into(),
            code,
            reason,
        }
    }

    /// The error name, as the upstream error class reports it.
    pub const fn name(&self) -> &'static str {
        "SSEHttpError"
    }

    /// The HTTP status code.
    pub const fn code(&self) -> u16 {
        self.code
    }

    /// The reason the error body named, sanitized and at most 128 UTF-16 units long.
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// Whether retrying cannot help: the server refused the credentials (401, 403) or does not know the session
    /// (404).
    pub fn is_permanent(&self) -> bool {
        PERMANENT_STATUSES.contains(&self.code)
    }
}

/// The reason an error response body names: the string at `error.resource`, else at `error.type`, from the first
/// 4096 bytes read as JSON. Control and format characters are removed and the text is cut to 128 UTF-16 units; a
/// body that does not say, or says it with something other than a string, has no reason.
pub fn http_error_reason(body: &[u8]) -> Option<String> {
    let prefix = &body[..body.len().min(BODY_LIMIT)];
    let mut decoder = SseUtf8Decoder::new();
    let text = decoder.decode(prefix, false);
    // A lone surrogate escape becomes a zero width space, which the strip below removes, as it removes the
    // surrogate itself.
    let parsed = parse_json(&text, "\\u200b").ok()?;
    let error = parsed.get("error")?.as_object()?;
    let named = error
        .get("resource")
        .filter(|value| !value.is_null())
        .or_else(|| error.get("type"));
    let Some(Value::String(reason)) = named else {
        return None;
    };
    let reason = truncate_utf16(&strip_invisible(reason), REASON_LIMIT);
    (!reason.is_empty()).then_some(reason)
}

#[cfg(test)]
#[path = "sse_http_error_tests.rs"]
mod tests;
