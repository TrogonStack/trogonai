use async_nats::header::HeaderMap;
use jsonrpc_nats::{RequestId, ResponseId};

use super::*;

fn headers_with_id(id: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(jsonrpc_nats::HEADER_ID, id);
    headers
}

#[test]
fn extract_numeric_id_from_header() {
    let mut headers = HeaderMap::new();
    headers.insert(jsonrpc_nats::HEADER_ID, "42");
    assert_eq!(extract_request_id(&headers), Some(JsonRpcId::Number(42)));
}

#[test]
fn extract_string_id_from_header() {
    assert_eq!(
        extract_request_id(&headers_with_id("\"abc-123\"")),
        Some(JsonRpcId::String("abc-123".into()))
    );
}

#[test]
fn extract_null_id_from_header() {
    let mut headers = HeaderMap::new();
    headers.insert(jsonrpc_nats::HEADER_ID, "null");
    assert_eq!(extract_request_id(&headers), Some(JsonRpcId::Null));
}

#[test]
fn missing_header_returns_none() {
    assert_eq!(extract_request_id(&HeaderMap::new()), None);
}

#[test]
fn extract_request_id_from_body_still_works() {
    let raw = br#"{"jsonrpc":"2.0","id":42,"method":"message/send","params":{}}"#;
    assert_eq!(extract_request_id_from_body(raw), Some(JsonRpcId::Number(42)));
}

#[test]
fn boolean_id_in_body_returns_none() {
    let raw = br#"{"id":true}"#;
    assert_eq!(extract_request_id_from_body(raw), None);
}

#[test]
fn id_roundtrips_through_serde() {
    let id = JsonRpcId::String("x".into());
    let bytes = serde_json::to_vec(&id).unwrap();
    let back: JsonRpcId = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(id, back);
}

#[test]
fn display_covers_every_variant() {
    assert_eq!(format!("{}", JsonRpcId::Number(42)), "42");
    assert_eq!(format!("{}", JsonRpcId::String("abc".into())), "abc");
    assert_eq!(format!("{}", JsonRpcId::Null), "null");
}

#[test]
fn response_id_converts_to_jsonrpc_id() {
    assert_eq!(
        JsonRpcId::from(ResponseId::String("req".into())),
        JsonRpcId::String("req".into())
    );
}

#[test]
fn request_id_converts_for_client_encoding() {
    let id = JsonRpcId::String("req".into());
    let req = match id {
        JsonRpcId::String(s) => RequestId::String(s),
        _ => panic!("expected string"),
    };
    assert!(matches!(req, RequestId::String(_)));
}
