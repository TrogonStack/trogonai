use std::fmt;

use crate::error::{
    AGENT_UNAVAILABLE, CONTENT_TYPE_NOT_SUPPORTED, INVALID_AGENT_RESPONSE, PUSH_NOTIFICATION_NOT_SUPPORTED,
    TASK_NOT_CANCELABLE, TASK_NOT_FOUND, UNSUPPORTED_OPERATION,
};

#[derive(Debug)]
pub enum ClientError {
    Serialize(serde_json::Error),
    Deserialize(serde_json::Error),
    Transport(String),
    Timeout {
        subject: String,
    },
    JetStream(String),
    TaskNotFound,
    TaskNotCancelable,
    PushNotificationNotSupported,
    UnsupportedOperation,
    ContentTypeNotSupported,
    InvalidAgentResponse,
    AgentUnavailable,
    JsonRpc {
        code: i32,
        message: String,
    },
    ConsumerSetup(String),
    StreamClosed,
    /// Returned when deriving a gateway ingress overlay from built-in agent subjects fails (internal invariant).
    InvalidRpcSubjectOverlay,
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
            AGENT_UNAVAILABLE => Self::AgentUnavailable,
            _ => Self::JsonRpc { code, message },
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(e) => write!(f, "failed to serialize request: {e}"),
            Self::Deserialize(e) => write!(f, "failed to deserialize response: {e}"),
            Self::Transport(msg) => write!(f, "transport error: {msg}"),
            Self::Timeout { subject } => write!(f, "request to '{subject}' timed out"),
            Self::JetStream(msg) => write!(f, "JetStream error: {msg}"),
            Self::TaskNotFound => write!(f, "task not found"),
            Self::TaskNotCancelable => write!(f, "task is not cancelable"),
            Self::PushNotificationNotSupported => write!(f, "push notifications not supported"),
            Self::UnsupportedOperation => write!(f, "operation not supported"),
            Self::ContentTypeNotSupported => write!(f, "content type not supported"),
            Self::InvalidAgentResponse => write!(f, "invalid agent response"),
            Self::AgentUnavailable => write!(f, "agent unavailable"),
            Self::JsonRpc { code, message } => write!(f, "JSON-RPC error {code}: {message}"),
            Self::ConsumerSetup(msg) => write!(f, "failed to set up event consumer: {msg}"),
            Self::StreamClosed => write!(f, "event stream closed unexpectedly"),
            Self::InvalidRpcSubjectOverlay => write!(f, "internal error deriving gateway ingress subject"),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(e) | Self::Deserialize(e) => Some(e),
            _ => None,
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
