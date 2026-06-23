use super::*;
use agent_client_protocol::{ErrorCode, RequestId};
use trogon_std::{FailNextSerialize, StdJsonSerialize};

#[test]
fn error_response_bytes_first_fallback_uses_null_id() {
    let mock = FailNextSerialize::new(1);
    let (bytes, content_type) =
        error_response_bytes(&mock, RequestId::Number(42), ErrorCode::InvalidParams, "test message");
    assert_eq!(content_type, "application/json");
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed["id"], serde_json::Value::Null);
    assert_eq!(parsed["error"]["code"], -32603);
}

#[test]
fn error_response_bytes_last_resort_returns_plain_text() {
    let mock = FailNextSerialize::new(2);
    let (bytes, content_type) = error_response_bytes(&mock, RequestId::Number(1), ErrorCode::InternalError, "msg");
    assert_eq!(content_type, "text/plain");
    assert_eq!(bytes.as_ref(), b"Internal error");
}

#[test]
fn error_response_fallback_bytes_std_serializer_returns_json() {
    let (bytes, content_type) = error_response_fallback_bytes(&StdJsonSerialize);
    assert_eq!(content_type, "application/json");
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed["id"], serde_json::Value::Null);
    assert_eq!(parsed["error"]["code"], -32603);
}
