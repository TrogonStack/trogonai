use crate::error::{
    AGENT_UNAVAILABLE, CONTENT_TYPE_NOT_SUPPORTED, EXTENDED_AGENT_CARD_NOT_CONFIGURED, EXTENSION_SUPPORT_REQUIRED,
    INVALID_AGENT_RESPONSE, PUSH_NOTIFICATION_NOT_SUPPORTED, TASK_NOT_CANCELABLE, TASK_NOT_FOUND,
    UNSUPPORTED_OPERATION, VERSION_NOT_SUPPORTED,
};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("failed to serialize request: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("failed to deserialize response: {0}")]
    Deserialize(#[source] serde_json::Error),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("request to '{subject}' timed out")]
    Timeout { subject: String },
    #[error("JetStream error: {0}")]
    JetStream(String),
    #[error("task not found")]
    TaskNotFound,
    #[error("task is not cancelable")]
    TaskNotCancelable,
    #[error("push notifications not supported")]
    PushNotificationNotSupported,
    #[error("operation not supported")]
    UnsupportedOperation,
    #[error("content type not supported")]
    ContentTypeNotSupported,
    #[error("invalid agent response")]
    InvalidAgentResponse,
    #[error("extended agent card not configured")]
    ExtendedAgentCardNotConfigured,
    #[error("extension support required: {0}")]
    ExtensionSupportRequired(String),
    #[error("A2A protocol version not supported: {0}")]
    VersionNotSupported(String),
    #[error("agent unavailable")]
    AgentUnavailable,
    #[error("JSON-RPC error {code}: {message}")]
    JsonRpc { code: i32, message: String },
    #[error("failed to set up event consumer: {0}")]
    ConsumerSetup(String),
    #[error("event stream closed unexpectedly")]
    StreamClosed,
    /// Returned when deriving a gateway ingress overlay from built-in agent subjects fails (internal invariant).
    #[error("internal error deriving gateway ingress subject")]
    InvalidRpcSubjectOverlay,
    /// Gateway ingress publish attempted with an expired minted User JWT (refresh before retrying).
    #[error("gateway caller JWT expired: {0}")]
    GatewayCallerJwtExpired(String),
    /// Minted User JWT failed freshness validation for a reason other than
    /// expiry (missing `exp`, not-yet-valid `nbf`, decode failure, clock skew).
    /// Callers should re-mint or investigate rather than treat as expired.
    #[error("gateway caller JWT failed freshness check: {0}")]
    GatewayCallerJwtInvalid(String),
}

impl ClientError {
    pub fn from_jsonrpc_code(code: i32, message: String) -> Self {
        match code {
            TASK_NOT_FOUND => Self::TaskNotFound,
            TASK_NOT_CANCELABLE => Self::TaskNotCancelable,
            PUSH_NOTIFICATION_NOT_SUPPORTED => Self::PushNotificationNotSupported,
            UNSUPPORTED_OPERATION => Self::UnsupportedOperation,
            CONTENT_TYPE_NOT_SUPPORTED => Self::ContentTypeNotSupported,
            INVALID_AGENT_RESPONSE => Self::InvalidAgentResponse,
            EXTENDED_AGENT_CARD_NOT_CONFIGURED => Self::ExtendedAgentCardNotConfigured,
            EXTENSION_SUPPORT_REQUIRED => Self::ExtensionSupportRequired(message),
            VERSION_NOT_SUPPORTED => Self::VersionNotSupported(message),
            AGENT_UNAVAILABLE => Self::AgentUnavailable,
            _ => Self::JsonRpc { code, message },
        }
    }
}

#[cfg(test)]
mod tests;
