use super::*;
use crate::direction::Direction;
use crate::{decode, encode};

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
