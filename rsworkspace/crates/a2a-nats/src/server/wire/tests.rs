use async_nats::header::HeaderMap;
use jsonrpc_nats::encode;

use super::*;
use crate::jsonrpc::JsonRpcId;
use crate::wire::{
    decode_request_params, encode_error, encode_success, is_notification, response_id_from_request_headers,
};

#[test]
fn parse_request_params_decodes_wire_request() {
    let encoded = encode(&jsonrpc_nats::Message::Request {
        id: jsonrpc_nats::RequestId::Number(1),
        method: "tasks/get".into(),
        params: serde_json::json!({"id": "t-1"}),
    })
    .unwrap();
    let params: serde_json::Value = parse_request_params("tasks/get", &encoded.headers, &encoded.body).unwrap();
    assert_eq!(params["id"], "t-1");
}

#[test]
fn is_notification_when_id_header_absent() {
    let encoded = encode(&jsonrpc_nats::Message::Notification {
        method: "tasks/get".into(),
        params: serde_json::json!({}),
    })
    .unwrap();
    assert!(is_notification(&encoded.headers));
}

#[test]
fn encode_success_reply_sets_result_body_only() {
    let mut headers = HeaderMap::new();
    headers.insert(jsonrpc_nats::HEADER_ID, "7");
    let encoded = encode_success(
        response_id_from_request_headers(&headers),
        &serde_json::json!({"ok": true}),
    )
    .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&encoded.body).unwrap();
    assert_eq!(body["ok"], true);
    assert!(encoded.headers.get(jsonrpc_nats::HEADER_ERROR_CODE).is_none());
}

#[test]
fn encode_error_reply_sets_error_code_header() {
    let mut headers = HeaderMap::new();
    headers.insert(jsonrpc_nats::HEADER_ID, "7");
    let encoded = encode_error(response_id_from_request_headers(&headers), -32001, "missing", None).unwrap();
    assert_eq!(
        encoded.headers.get(jsonrpc_nats::HEADER_ERROR_CODE).unwrap().as_str(),
        "-32001"
    );
}

#[test]
fn optional_id_states_decode_from_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(jsonrpc_nats::HEADER_ID, "null");
    assert_eq!(crate::jsonrpc::extract_request_id(&headers), Some(JsonRpcId::Null));
}

#[test]
fn malformed_body_still_allows_id_recovery_from_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(jsonrpc_nats::HEADER_ID, "9");
    let err = decode_request_params::<serde_json::Value>("tasks/get", &headers, b"{");
    assert!(err.is_err());
}

#[test]
fn encode_success_reply_with_no_id_header_produces_null_id() {
    let headers = HeaderMap::new();
    let encoded = encode_success_reply(&headers, &serde_json::json!({"result": true})).unwrap();
    let body: serde_json::Value = serde_json::from_slice(&encoded.body).unwrap();
    assert_eq!(body["result"], true);
}

#[test]
fn encode_error_reply_with_no_id_header_produces_null_id() {
    let headers = HeaderMap::new();
    let encoded = encode_error_reply(&headers, -32600, "invalid request", None).unwrap();
    assert_eq!(
        encoded.headers.get(jsonrpc_nats::HEADER_ERROR_CODE).unwrap().as_str(),
        "-32600"
    );
}

#[test]
fn request_id_returns_none_when_header_absent() {
    let headers = HeaderMap::new();
    assert_eq!(request_id(&headers), None);
}

#[test]
fn request_id_returns_some_when_header_present() {
    let mut headers = HeaderMap::new();
    headers.insert(jsonrpc_nats::HEADER_ID, "5");
    assert_eq!(request_id(&headers), Some(JsonRpcId::Number(5)));
}
