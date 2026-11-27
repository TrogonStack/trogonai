use serde::{Deserialize, Serialize};

/// The error half of a response frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsErrorBody {
    pub code: String,
    pub message: String,
    /// Stable snake_case error variant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// A retry after a backoff may succeed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    /// The request may already have been applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_unknown: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
    /// How long to wait before retrying, when the denial names a delay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

impl WsErrorBody {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            kind: None,
            retryable: None,
            outcome_unknown: None,
            hint: None,
            provider: None,
            lease_id: None,
            retry_after_ms: None,
        }
    }
}
