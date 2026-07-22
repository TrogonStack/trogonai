mod prop_tests;

use super::*;
use crate::constants::{HEADER_ERROR_CODE, HEADER_ID};
use crate::direction::Direction;
use crate::error::CodecError;
use crate::id::encode_id_literal;
use crate::{decode, encode, from_json_value};
use async_nats::header::HeaderMap;

#[test]
fn from_json_value_rejects_mismatched_version() {
    let v3 = serde_json::json!({ "jsonrpc": "3.0", "id": 1, "method": "ping", "params": {} });
    assert!(matches!(
        from_json_value(&v3),
        Err(CodecError::UnsupportedVersion { found }) if found.as_deref() == Some("3.0")
    ));
}

#[test]
fn from_json_value_rejects_missing_version() {
    let missing = serde_json::json!({ "id": 1, "method": "ping", "params": {} });
    assert!(matches!(
        from_json_value(&missing),
        Err(CodecError::UnsupportedVersion { found: None })
    ));
}

#[test]
fn numeric_and_string_ids_are_distinct_on_the_wire() {
    let numeric = Message::Request {
        id: RequestId::Number(1),
        method: "ping".to_string(),
        params: serde_json::json!({}),
    };
    let string = Message::Request {
        id: RequestId::String("1".to_string()),
        method: "ping".to_string(),
        params: serde_json::json!({}),
    };

    let numeric_wire = encode(&numeric).unwrap();
    let string_wire = encode(&string).unwrap();

    assert_eq!(numeric_wire.headers.get(HEADER_ID).unwrap().as_str(), "1");
    assert_eq!(string_wire.headers.get(HEADER_ID).unwrap().as_str(), "\"1\"");
}

#[test]
fn error_response_is_discriminated_by_error_code_header() {
    let message = Message::Error {
        id: ResponseId::Number(9),
        code: -32000,
        message: "auth failed".to_string(),
        data: Some(serde_json::json!({"reason": "expired"})),
    };

    let wire = encode(&message).unwrap();
    assert_eq!(wire.headers.get(HEADER_ERROR_CODE).unwrap().as_str(), "-32000");
    let body: serde_json::Value = serde_json::from_slice(&wire.body).unwrap();
    assert_eq!(body["message"], "auth failed");
    assert_eq!(body["data"]["reason"], "expired");

    let decoded = decode(Direction::Response, None, &wire.headers, &wire.body).unwrap();
    assert_eq!(decoded, message);
}

#[test]
fn absent_id_on_response_means_null() {
    let message = Message::Success {
        id: ResponseId::Null,
        result: serde_json::json!(true),
    };
    let wire = encode(&message).unwrap();
    assert!(wire.headers.get(HEADER_ID).is_none());

    let decoded = decode(Direction::Response, None, &wire.headers, &wire.body).unwrap();
    assert_eq!(decoded, message);
}

#[test]
fn absent_id_on_request_means_notification() {
    let message = Message::Notification {
        method: "notify".to_string(),
        params: serde_json::json!({"x": 1}),
    };
    let wire = encode(&message).unwrap();
    assert!(wire.headers.get(HEADER_ID).is_none());

    let decoded = decode(Direction::Request, Some("notify"), &wire.headers, &wire.body).unwrap();
    assert_eq!(decoded, message);
}

#[test]
fn ambiguous_response_without_result_or_error_code_is_rejected() {
    let headers = async_nats::HeaderMap::new();
    let err = decode(Direction::Response, None, &headers, &[]).unwrap_err();
    assert!(matches!(err, CodecError::AmbiguousResponse));
}

#[test]
fn round_trip_via_json_value() {
    let original = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "unicode-\u{1F600}",
        "method": "test",
        "params": {"n": 42}
    });
    let message = from_json_value(&original).unwrap();
    let roundtrip = to_json_value(&message);
    assert_eq!(roundtrip, original);
}

#[test]
fn decode_request_with_id_and_empty_body_has_null_params() {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_ID, encode_id_literal(&RequestId::Number(1)));
    let msg = decode(Direction::Request, Some("ping"), &headers, &[]).unwrap();
    assert!(matches!(msg, Message::Request { params, .. } if params.is_null()));
}

#[test]
fn decode_request_without_id_but_error_code_is_ambiguous() {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_ERROR_CODE, "1");
    let err = decode(Direction::Request, Some("ping"), &headers, &[]).unwrap_err();
    assert!(matches!(err, CodecError::AmbiguousResponse));
}

#[test]
fn decode_request_without_id_and_empty_body_is_notification() {
    let headers = HeaderMap::new();
    let msg = decode(Direction::Request, Some("notify"), &headers, &[]).unwrap();
    assert!(matches!(msg, Message::Notification { params, .. } if params.is_null()));
}

#[test]
fn decode_response_error_with_empty_body_has_empty_message() {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_ERROR_CODE, "42");
    let msg = decode(Direction::Response, None, &headers, &[]).unwrap();
    assert!(matches!(msg, Message::Error { message, data, .. } if message.is_empty() && data.is_none()));
}

#[test]
fn decode_response_error_body_must_be_object() {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_ERROR_CODE, "42");
    let err = decode(Direction::Response, None, &headers, b"\"scalar\"").unwrap_err();
    assert!(matches!(err, CodecError::Deserialize(_)));
}

#[test]
fn from_json_value_rejects_request_with_null_id() {
    let value = serde_json::json!({ "jsonrpc": "2.0", "method": "ping", "id": null, "params": {} });
    assert!(matches!(from_json_value(&value), Err(CodecError::RequestWithoutId)));
}

#[test]
fn from_json_value_parses_request_with_id() {
    let value = serde_json::json!({ "jsonrpc": "2.0", "method": "ping", "id": 7, "params": { "a": 1 } });
    assert!(matches!(from_json_value(&value).unwrap(), Message::Request { method, .. } if method == "ping"));
}

#[test]
fn from_json_value_rejects_error_without_code() {
    let value = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "error": { "message": "boom" } });
    assert!(matches!(from_json_value(&value), Err(CodecError::Deserialize(_))));
}
