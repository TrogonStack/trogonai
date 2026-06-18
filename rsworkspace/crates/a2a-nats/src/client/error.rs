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
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn task_not_found_code_maps_correctly() {
        let err = ClientError::from_jsonrpc_code(TASK_NOT_FOUND, "not found".into());
        assert!(matches!(err, ClientError::TaskNotFound));
    }

    #[test]
    fn task_not_cancelable_code_maps_correctly() {
        let err = ClientError::from_jsonrpc_code(TASK_NOT_CANCELABLE, "not cancelable".into());
        assert!(matches!(err, ClientError::TaskNotCancelable));
    }

    #[test]
    fn push_not_supported_code_maps_correctly() {
        let err = ClientError::from_jsonrpc_code(PUSH_NOTIFICATION_NOT_SUPPORTED, "no push".into());
        assert!(matches!(err, ClientError::PushNotificationNotSupported));
    }

    #[test]
    fn unsupported_operation_code_maps_correctly() {
        let err = ClientError::from_jsonrpc_code(UNSUPPORTED_OPERATION, "unsupported".into());
        assert!(matches!(err, ClientError::UnsupportedOperation));
    }

    #[test]
    fn content_type_not_supported_code_maps_correctly() {
        let err = ClientError::from_jsonrpc_code(CONTENT_TYPE_NOT_SUPPORTED, "bad type".into());
        assert!(matches!(err, ClientError::ContentTypeNotSupported));
    }

    #[test]
    fn invalid_agent_response_code_maps_correctly() {
        let err = ClientError::from_jsonrpc_code(INVALID_AGENT_RESPONSE, "invalid".into());
        assert!(matches!(err, ClientError::InvalidAgentResponse));
    }

    #[test]
    fn agent_unavailable_code_maps_correctly() {
        let err = ClientError::from_jsonrpc_code(AGENT_UNAVAILABLE, "unavailable".into());
        assert!(matches!(err, ClientError::AgentUnavailable));
    }

    #[test]
    fn extended_card_not_configured_code_maps_correctly() {
        let err = ClientError::from_jsonrpc_code(EXTENDED_AGENT_CARD_NOT_CONFIGURED, "no ext card".into());
        assert!(matches!(err, ClientError::ExtendedAgentCardNotConfigured));
    }

    #[test]
    fn extension_support_required_code_carries_message() {
        let err = ClientError::from_jsonrpc_code(EXTENSION_SUPPORT_REQUIRED, "need ext foo".into());
        match err {
            ClientError::ExtensionSupportRequired(msg) => assert_eq!(msg, "need ext foo"),
            other => panic!("unexpected variant {other:?}"),
        }
    }

    #[test]
    fn version_not_supported_code_carries_message() {
        let err = ClientError::from_jsonrpc_code(VERSION_NOT_SUPPORTED, "9.9.9".into());
        match err {
            ClientError::VersionNotSupported(msg) => assert_eq!(msg, "9.9.9"),
            other => panic!("unexpected variant {other:?}"),
        }
    }

    #[test]
    fn unknown_code_maps_to_generic_jsonrpc() {
        let err = ClientError::from_jsonrpc_code(-32099, "custom error".into());
        assert!(matches!(err, ClientError::JsonRpc { code: -32099, .. }));
    }

    #[test]
    fn display_serialize() {
        let err = ClientError::Serialize(serde_json::from_str::<String>("x").unwrap_err());
        assert!(err.to_string().contains("serialize request"));
    }

    #[test]
    fn display_deserialize() {
        let err = ClientError::Deserialize(serde_json::from_str::<String>("x").unwrap_err());
        assert!(err.to_string().contains("deserialize response"));
    }

    #[test]
    fn display_transport() {
        let err = ClientError::Transport("conn reset".into());
        assert!(err.to_string().contains("transport error: conn reset"));
    }

    #[test]
    fn display_timeout() {
        let err = ClientError::Timeout {
            subject: "a.b.c".into(),
        };
        assert!(err.to_string().contains("'a.b.c' timed out"));
    }

    #[test]
    fn display_jetstream() {
        let err = ClientError::JetStream("no stream".into());
        assert!(err.to_string().contains("JetStream error"));
    }

    #[test]
    fn display_task_not_found() {
        assert!(ClientError::TaskNotFound.to_string().contains("task not found"));
    }

    #[test]
    fn display_task_not_cancelable() {
        assert!(ClientError::TaskNotCancelable.to_string().contains("not cancelable"));
    }

    #[test]
    fn display_push_not_supported() {
        assert!(ClientError::PushNotificationNotSupported.to_string().contains("push"));
    }

    #[test]
    fn display_unsupported_op() {
        assert!(ClientError::UnsupportedOperation.to_string().contains("not supported"));
    }

    #[test]
    fn display_content_type() {
        assert!(
            ClientError::ContentTypeNotSupported
                .to_string()
                .contains("content type")
        );
    }

    #[test]
    fn display_invalid_agent_response() {
        assert!(ClientError::InvalidAgentResponse.to_string().contains("invalid agent"));
    }

    #[test]
    fn display_agent_unavailable() {
        assert!(ClientError::AgentUnavailable.to_string().contains("unavailable"));
    }

    #[test]
    fn display_extended_card_not_configured() {
        assert!(
            ClientError::ExtendedAgentCardNotConfigured
                .to_string()
                .contains("extended agent card")
        );
    }

    #[test]
    fn display_extension_support_required() {
        let err = ClientError::ExtensionSupportRequired("foo".into());
        assert!(err.to_string().contains("extension support required"));
        assert!(err.to_string().contains("foo"));
    }

    #[test]
    fn display_version_not_supported() {
        let err = ClientError::VersionNotSupported("0.4".into());
        assert!(err.to_string().contains("version not supported"));
        assert!(err.to_string().contains("0.4"));
    }

    #[test]
    fn display_jsonrpc_generic() {
        let err = ClientError::JsonRpc {
            code: -32001,
            message: "oops".into(),
        };
        assert!(err.to_string().contains("-32001"));
        assert!(err.to_string().contains("oops"));
    }

    #[test]
    fn display_consumer_setup() {
        let err = ClientError::ConsumerSetup("no stream".into());
        assert!(err.to_string().contains("consumer"));
    }

    #[test]
    fn display_stream_closed() {
        assert!(ClientError::StreamClosed.to_string().contains("closed"));
    }

    #[test]
    fn display_invalid_rpc_subject_overlay() {
        assert!(
            ClientError::InvalidRpcSubjectOverlay
                .to_string()
                .contains("gateway ingress")
        );
    }

    #[test]
    fn display_gateway_caller_jwt_expired() {
        let err = ClientError::GatewayCallerJwtExpired("user JWT expired".into());
        assert!(err.to_string().contains("expired"));
    }

    #[test]
    fn display_gateway_caller_jwt_invalid() {
        let err = ClientError::GatewayCallerJwtInvalid("user JWT missing exp".into());
        assert!(err.to_string().contains("failed freshness check"));
    }

    #[test]
    fn error_source_for_serialize() {
        let e = ClientError::Serialize(serde_json::from_str::<String>("x").unwrap_err());
        assert!(e.source().is_some());
    }

    #[test]
    fn error_source_for_transport() {
        let e = ClientError::Transport("err".into());
        assert!(e.source().is_none());
    }
}
