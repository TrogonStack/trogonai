use super::*;
use agent_client_protocol::{Error, ErrorCode};
use jsonrpc_nats::ResponseId;

#[test]
fn encode_success_sets_jsonrpc_id_header() {
    let encoded = encode_success_for_test(ResponseId::Number(42), &serde_json::json!({"ok": true})).unwrap();
    assert_eq!(encoded.headers.get(jsonrpc_nats::HEADER_ID).unwrap().as_str(), "42");
    assert!(encoded.headers.get(jsonrpc_nats::HEADER_ERROR_CODE).is_none());
}

#[test]
fn encode_agent_error_sets_error_code_header() {
    let error = Error::new(ErrorCode::InvalidParams.into(), "test message");
    let encoded = encode_agent_error_for_test(ResponseId::Number(1), &error).unwrap();
    assert_eq!(
        encoded.headers.get(jsonrpc_nats::HEADER_ERROR_CODE).unwrap().as_str(),
        "-32602"
    );
    assert_eq!(encoded.headers.get(jsonrpc_nats::HEADER_ID).unwrap().as_str(), "1");
}

#[test]
fn encode_agent_error_null_id() {
    let error = Error::new(ErrorCode::InternalError.into(), "Internal error");
    let encoded = encode_agent_error_for_test(ResponseId::Null, &error).unwrap();
    assert_eq!(encoded.headers.get(jsonrpc_nats::HEADER_ID).unwrap().as_str(), "null");
}
