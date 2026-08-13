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
    assert_eq!(extract_request_id(&headers), Some(ResponseId::Number(42)));
}

#[test]
fn extract_string_id_from_header() {
    assert_eq!(
        extract_request_id(&headers_with_id("\"abc-123\"")),
        Some(ResponseId::String("abc-123".into()))
    );
}

#[test]
fn extract_null_id_from_header() {
    let mut headers = HeaderMap::new();
    headers.insert(jsonrpc_nats::HEADER_ID, "null");
    assert_eq!(extract_request_id(&headers), Some(ResponseId::Null));
}

#[test]
fn missing_header_returns_none() {
    assert_eq!(extract_request_id(&HeaderMap::new()), None);
}

#[test]
fn extract_request_id_from_body_still_works() {
    let raw = br#"{"jsonrpc":"2.0","id":42,"method":"message/send","params":{}}"#;
    assert_eq!(extract_request_id_from_body(raw), Some(ResponseId::Number(42)));
}

#[test]
fn boolean_id_in_body_returns_none() {
    let raw = br#"{"id":true}"#;
    assert_eq!(extract_request_id_from_body(raw), None);
}

#[test]
fn id_roundtrips_through_serde() {
    let id = ResponseId::String("x".into());
    let bytes = serde_json::to_vec(&id).unwrap();
    let back: ResponseId = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(id, back);
}

/// A request id may not be null, so the request path takes `RequestId`, which has
/// no null variant, and crossing from request to response is infallible.
#[test]
fn a_request_id_is_always_a_valid_response_id() {
    assert_eq!(
        ResponseId::from(RequestId::String("req".into())),
        ResponseId::String("req".into())
    );
    assert!(RequestId::try_from(ResponseId::Null).is_err());
}
