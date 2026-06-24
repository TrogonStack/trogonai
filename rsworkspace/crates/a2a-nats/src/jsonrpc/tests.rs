use super::*;

#[test]
fn extract_numeric_id() {
    let raw = br#"{"jsonrpc":"2.0","id":42,"method":"message/send","params":{}}"#;
    assert_eq!(extract_request_id(raw), Some(JsonRpcId::Number(42)));
}

#[test]
fn extract_string_id() {
    let raw = br#"{"id":"abc-123"}"#;
    assert_eq!(extract_request_id(raw), Some(JsonRpcId::String("abc-123".into())));
}

#[test]
fn extract_null_id() {
    let raw = br#"{"id":null}"#;
    assert_eq!(extract_request_id(raw), Some(JsonRpcId::Null));
}

#[test]
fn missing_id_returns_none() {
    let raw = br#"{"jsonrpc":"2.0"}"#;
    assert_eq!(extract_request_id(raw), None);
}

#[test]
fn malformed_payload_returns_none() {
    assert_eq!(extract_request_id(b"not json"), None);
}

#[test]
fn boolean_id_returns_none() {
    let raw = br#"{"id":true}"#;
    assert_eq!(extract_request_id(raw), None);
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
