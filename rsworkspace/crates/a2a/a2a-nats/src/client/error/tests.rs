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
    let inner = serde_json::from_str::<String>("x").unwrap_err();
    let expected = format!("failed to serialize request: {inner}");
    let err = ClientError::Serialize(inner);
    assert_eq!(err.to_string(), expected);
}

#[test]
fn display_deserialize() {
    let inner = serde_json::from_str::<String>("x").unwrap_err();
    let expected = format!("failed to deserialize response: {inner}");
    let err = ClientError::Deserialize(inner);
    assert_eq!(err.to_string(), expected);
}

#[test]
fn display_transport() {
    let err = ClientError::Transport("conn reset".into());
    assert_eq!(err.to_string(), "transport error: conn reset");
}

#[test]
fn display_timeout() {
    let err = ClientError::Timeout {
        subject: "a.b.c".into(),
    };
    assert_eq!(err.to_string(), "request to 'a.b.c' timed out");
}

#[test]
fn display_jetstream() {
    let err = ClientError::JetStream("no stream".into());
    assert_eq!(err.to_string(), "JetStream error: no stream");
}

#[test]
fn display_task_not_found() {
    assert_eq!(ClientError::TaskNotFound.to_string(), "task not found");
}

#[test]
fn display_task_not_cancelable() {
    assert_eq!(ClientError::TaskNotCancelable.to_string(), "task is not cancelable");
}

#[test]
fn display_push_not_supported() {
    assert_eq!(
        ClientError::PushNotificationNotSupported.to_string(),
        "push notifications not supported"
    );
}

#[test]
fn display_unsupported_op() {
    assert_eq!(ClientError::UnsupportedOperation.to_string(), "operation not supported");
}

#[test]
fn display_content_type() {
    assert_eq!(
        ClientError::ContentTypeNotSupported.to_string(),
        "content type not supported"
    );
}

#[test]
fn display_invalid_agent_response() {
    assert_eq!(ClientError::InvalidAgentResponse.to_string(), "invalid agent response");
}

#[test]
fn display_agent_unavailable() {
    assert_eq!(ClientError::AgentUnavailable.to_string(), "agent unavailable");
}

#[test]
fn display_extended_card_not_configured() {
    assert_eq!(
        ClientError::ExtendedAgentCardNotConfigured.to_string(),
        "extended agent card not configured"
    );
}

#[test]
fn display_extension_support_required() {
    let err = ClientError::ExtensionSupportRequired("foo".into());
    assert_eq!(err.to_string(), "extension support required: foo");
}

#[test]
fn display_version_not_supported() {
    let err = ClientError::VersionNotSupported("0.4".into());
    assert_eq!(err.to_string(), "A2A protocol version not supported: 0.4");
}

#[test]
fn display_jsonrpc_generic() {
    let err = ClientError::JsonRpc {
        code: -32001,
        message: "oops".into(),
    };
    assert_eq!(err.to_string(), "JSON-RPC error -32001: oops");
}

#[test]
fn display_consumer_setup() {
    let err = ClientError::ConsumerSetup("no stream".into());
    assert_eq!(err.to_string(), "failed to set up event consumer: no stream");
}

#[test]
fn display_stream_closed() {
    assert_eq!(
        ClientError::StreamClosed.to_string(),
        "event stream closed unexpectedly"
    );
}

#[test]
fn display_invalid_rpc_subject_overlay() {
    assert_eq!(
        ClientError::InvalidRpcSubjectOverlay.to_string(),
        "internal error deriving gateway ingress subject"
    );
}

#[test]
fn display_gateway_caller_jwt_expired() {
    let err = ClientError::GatewayCallerJwtExpired("user JWT expired".into());
    assert_eq!(err.to_string(), "gateway caller JWT expired: user JWT expired");
}

#[test]
fn display_gateway_caller_jwt_invalid() {
    let err = ClientError::GatewayCallerJwtInvalid("user JWT missing exp".into());
    assert_eq!(
        err.to_string(),
        "gateway caller JWT failed freshness check: user JWT missing exp"
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
