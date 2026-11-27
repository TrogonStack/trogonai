use serde_json::Value;

/// Why a SPACE call failed, split by what the caller may conclude about whether the
/// request took effect.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// SPACE answered with an error envelope. `retryable` is SPACE's own statement
    /// of whether sending the same request again can succeed.
    #[error("SPACE API returned HTTP {status} {code}: {message}{}", request_suffix(.request_id))]
    Status {
        /// The HTTP status.
        status: u16,
        /// The SPACE error code.
        code: String,
        /// The message SPACE sent.
        message: String,
        /// Whether the same request may succeed when repeated.
        retryable: bool,
        /// The request identifier SPACE assigned.
        request_id: Option<String>,
        /// The machine-readable details.
        details: Box<Value>,
    },
    /// The connection was never established, so the request did nothing.
    #[error("SPACE API was unreachable: {0}")]
    NotApplied(String),
    /// The request may or may not have been processed: it timed out, the connection
    /// dropped, or a success body could not be read.
    #[error("SPACE API call has an unknown outcome: {0}")]
    Ambiguous(String),
    /// SPACE answered with something this client does not understand.
    #[error("SPACE API reply was not understood: {0}")]
    Protocol(String),
}

impl ApiError {
    /// SPACE reports the resource as absent.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Status { status, code, .. } if *status == 404 || code == "not_found")
    }

    /// The request definitely did not take effect, as opposed to a failure whose
    /// effect is unknown.
    pub fn definitely_not_applied(&self) -> bool {
        match self {
            Self::NotApplied(_) => true,
            Self::Status { status, .. } => (400..500).contains(status) && *status != 408,
            Self::Ambiguous(_) | Self::Protocol(_) => false,
        }
    }

    /// The same request, resent unchanged with the same idempotency key, may succeed.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::NotApplied(_) | Self::Ambiguous(_) => true,
            Self::Status { retryable, .. } => *retryable,
            Self::Protocol(_) => false,
        }
    }

    /// The SPACE error code, when SPACE sent one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Status { code, .. } => Some(code),
            Self::NotApplied(_) | Self::Ambiguous(_) | Self::Protocol(_) => None,
        }
    }

    /// The machine-readable reason in the envelope details, such as
    /// `sandbox_not_running`.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Status { details, .. } => details.as_array()?.iter().find_map(|detail| detail["reason"].as_str()),
            Self::NotApplied(_) | Self::Ambiguous(_) | Self::Protocol(_) => None,
        }
    }
}

fn request_suffix(request_id: &Option<String>) -> String {
    request_id
        .as_ref()
        .map(|id| format!(" (request {id})"))
        .unwrap_or_default()
}
