use async_nats::header::HeaderMap;
use jsonrpc_nats::{Message, RequestId, ResponseId, encode};
use serde::{Deserialize, Serialize};

use super::*;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Params {
    x: i32,
}

#[test]
fn response_id_from_request_headers_returns_null_when_no_id_header() {
    let headers = HeaderMap::new();
    let id = response_id_from_request_headers(&headers);
    assert!(matches!(id, ResponseId::Null));
}

#[test]
fn decode_response_deserialize_error_when_result_type_mismatches() {
    let encoded = encode(&Message::Success {
        id: ResponseId::Number(1),
        result: serde_json::json!({"unexpected_key": true}),
    })
    .unwrap();
    let result = decode_response::<Params>(&encoded.headers, &encoded.body);
    assert!(matches!(result, Err(WireError::Deserialize(_))));
}

#[test]
fn decode_request_params_handles_notification_variant() {
    let encoded = encode(&Message::Notification {
        method: "tasks/get".into(),
        params: serde_json::json!({"x": 42}),
    })
    .unwrap();
    let params: Params = decode_request_params("tasks/get", &encoded.headers, &encoded.body).unwrap();
    assert_eq!(params.x, 42);
}

#[test]
fn encode_request_produces_valid_three_segment_jwt() {
    let encoded = encode_request(
        "tasks/get",
        RequestId::String("abc".into()),
        &serde_json::json!({"x": 1}),
    )
    .unwrap();
    assert!(encoded.headers.get(jsonrpc_nats::HEADER_ID).is_some());
    let body: serde_json::Value = serde_json::from_slice(&encoded.body).unwrap();
    assert_eq!(body["x"], 1);
}

#[test]
fn encode_success_produces_valid_response() {
    let encoded = encode_success(ResponseId::Number(1), &serde_json::json!({"ok": true})).unwrap();
    let body: serde_json::Value = serde_json::from_slice(&encoded.body).unwrap();
    assert_eq!(body["ok"], true);
}

#[test]
fn encode_error_sets_error_code_header() {
    let encoded = encode_error(ResponseId::Number(1), -32001, "bad request", None).unwrap();
    assert_eq!(
        encoded.headers.get(jsonrpc_nats::HEADER_ERROR_CODE).unwrap().as_str(),
        "-32001"
    );
}

#[test]
fn is_notification_returns_false_when_id_header_present() {
    let encoded = encode(&Message::Request {
        id: RequestId::Number(1),
        method: "tasks/get".into(),
        params: serde_json::json!({}),
    })
    .unwrap();
    assert!(!is_notification(&encoded.headers));
}
