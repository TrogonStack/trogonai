use super::*;
use agent_client_protocol::ErrorCode;
use trogon_nats::{NatsError, PublishOperationError};

#[test]
fn timeout_returns_agent_unavailable() {
    let err = map_nats_error(NatsError::Timeout {
        subject: "test.subject".into(),
    });
    assert_eq!(err.message, "Request timed out; agent may be overloaded or unavailable");
    assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
}

#[test]
fn request_error_returns_agent_unavailable() {
    let err = map_nats_error(NatsError::Request {
        subject: "test.subject".into(),
        error: "connection refused".into(),
    });
    assert_eq!(err.message, "Agent unavailable: connection refused");
    assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
}

#[test]
fn serialize_error_returns_internal() {
    let serde_err = serde_json::to_vec(&FailsSerialize).unwrap_err();
    let err = map_nats_error(NatsError::Serialize(serde_err));
    assert_eq!(err.message, "Failed to serialize request: test serialize failure");
    assert_eq!(err.code, ErrorCode::InternalError);
}

#[test]
fn deserialize_error_returns_internal() {
    let serde_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
    let err = map_nats_error(NatsError::Deserialize(serde_err));
    assert_eq!(err.message, "Invalid response from agent");
    assert_eq!(err.code, ErrorCode::InternalError);
}

#[test]
fn publish_operation_returns_internal() {
    let err = map_nats_error(NatsError::PublishOperation(PublishOperationError(
        "flush failed".into(),
    )));
    assert_eq!(err.message, "Publish operation failed");
    assert_eq!(err.code, ErrorCode::InternalError);
}

#[test]
fn publish_operation_exhausted_returns_internal() {
    let err = map_nats_error(NatsError::PublishOperationExhausted {
        error: PublishOperationError("flush failed".into()),
        subject: "test.subject".into(),
        attempts: 3,
    });
    assert_eq!(err.message, "Publish operation exhausted");
    assert_eq!(err.code, ErrorCode::InternalError);
}

#[test]
fn other_error_returns_internal() {
    let err = map_nats_error(NatsError::Other("misc failure".into()));
    assert_eq!(err.message, "Request failed");
    assert_eq!(err.code, ErrorCode::InternalError);
}

struct FailsSerialize;
impl serde::Serialize for FailsSerialize {
    fn serialize<S: serde::Serializer>(&self, _s: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("test serialize failure"))
    }
}
