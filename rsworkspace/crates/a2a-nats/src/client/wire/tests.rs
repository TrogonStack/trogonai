use async_nats::header::HeaderMap;
use jsonrpc_nats::{Direction, Message, RequestId, ResponseId, decode, encode, to_json_value};
use serde::{Deserialize, Serialize};

use super::*;
use crate::jsonrpc::JsonRpcId;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct DummyParams {
    value: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct DummyResult {
    answer: i32,
}

#[test]
fn encode_client_request_puts_id_in_header_and_params_in_body() {
    let encoded = encode_client_request(
        "tasks/get",
        JsonRpcId::Number(1),
        &DummyParams {
            value: "x".into(),
        },
    )
    .unwrap();
    assert!(encoded.headers.get(jsonrpc_nats::HEADER_ID).is_some());
    let params: DummyParams = serde_json::from_slice(&encoded.body).unwrap();
    assert_eq!(params.value, "x");
}

#[test]
fn decode_client_success_response() {
    let encoded = encode(&Message::Success {
        id: ResponseId::Number(1),
        result: serde_json::json!({"answer": 42}),
    })
    .unwrap();
    let result: DummyResult = decode_client_response(&encoded.headers, &encoded.body)
        .unwrap()
        .unwrap();
    assert_eq!(result.answer, 42);
}

#[test]
fn decode_client_error_response() {
    let encoded = encode(&Message::Error {
        id: ResponseId::Number(1),
        code: -32001,
        message: "nope".into(),
        data: None,
    })
    .unwrap();
    let err = decode_client_response::<DummyResult>(&encoded.headers, &encoded.body)
        .unwrap()
        .unwrap_err();
    assert_eq!(err, (-32001, "nope".to_string()));
}

#[test]
fn roundtrip_reconstructs_canonical_json_at_edge() {
    let encoded = encode_client_request(
        "tasks/get",
        JsonRpcId::String("abc".into()),
        &DummyParams {
            value: "x".into(),
        },
    )
    .unwrap();
    let message = decode(Direction::Request, Some("tasks/get"), &encoded.headers, &encoded.body).unwrap();
    let value = to_json_value(&message);
    assert_eq!(value["id"], "abc");
    assert_eq!(value["method"], "tasks/get");
}

#[test]
fn merge_headers_overlays_jsonrpc_fields() {
    let mut base = HeaderMap::new();
    base.insert("X-Req-Id", "transport");
    let encoded = encode(&Message::Request {
        id: RequestId::String("abc".into()),
        method: "tasks/get".into(),
        params: serde_json::json!({}),
    })
    .unwrap();
    let merged = merge_jsonrpc_headers(base, encoded.headers);
    assert_eq!(merged.get("X-Req-Id").unwrap().as_str(), "transport");
    assert!(merged.get(jsonrpc_nats::HEADER_ID).is_some());
}
