use super::*;

#[test]
fn parse_valid_request() {
    let raw = br#"{"jsonrpc":"2.0","id":1,"method":"message/send","params":{"key":"val"}}"#;
    let req: JsonRpcRequest<serde_json::Value> = parse_request(raw).unwrap();
    assert_eq!(req.version, "2.0");
    assert_eq!(req.id, Some(JsonRpcId::Number(1)));
    assert_eq!(req.method.as_deref(), Some("message/send"));
}

#[test]
fn parse_request_missing_params() {
    let raw = br#"{"jsonrpc":"2.0","id":1,"method":"tasks/get"}"#;
    let req: JsonRpcRequest<serde_json::Value> = parse_request(raw).unwrap();
    assert!(req.params.is_none());
}

#[test]
fn parse_request_malformed() {
    let raw = b"not json";
    let result: Result<JsonRpcRequest<serde_json::Value>, _> = parse_request(raw);
    assert!(result.is_err());
}

#[test]
fn response_serializes_with_2_0_version() {
    let resp = JsonRpcResponse::new(Some(JsonRpcId::Number(1)), serde_json::json!({"ok": true}));
    let bytes = resp.to_bytes().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["id"], 1);
    assert_eq!(v["result"]["ok"], true);
}

#[test]
fn error_response_round_trips() {
    let resp = JsonRpcErrorResponse::new(Some(JsonRpcId::String("x".into())), -32001, "not found");
    let bytes = resp.to_bytes().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["error"]["code"], -32001);
    assert_eq!(v["error"]["message"], "not found");
    assert_eq!(v["id"], "x");
}

#[test]
fn parse_error_response_has_32700() {
    let resp = JsonRpcErrorResponse::parse_error(None);
    assert_eq!(resp.error.code, -32700);
}

#[test]
fn internal_error_response() {
    let resp = JsonRpcErrorResponse::internal_error(None, "boom");
    assert_eq!(resp.error.code, -32603);
    assert_eq!(resp.error.message, "boom");
}

#[test]
fn request_absent_id_is_notification() {
    let raw = br#"{"jsonrpc":"2.0","method":"m","params":null}"#;
    let req: JsonRpcRequest<serde_json::Value> = parse_request(raw).unwrap();
    assert_eq!(req.id, None);
}

#[test]
fn request_explicit_null_id_is_call_with_null() {
    let raw = br#"{"jsonrpc":"2.0","id":null,"method":"m","params":null}"#;
    let req: JsonRpcRequest<serde_json::Value> = parse_request(raw).unwrap();
    assert_eq!(req.id, Some(JsonRpcId::Null));
}

#[test]
fn request_number_id_round_trips() {
    let raw = br#"{"jsonrpc":"2.0","id":42,"method":"m","params":null}"#;
    let req: JsonRpcRequest<serde_json::Value> = parse_request(raw).unwrap();
    assert_eq!(req.id, Some(JsonRpcId::Number(42)));
}

#[test]
fn request_string_id_round_trips() {
    let raw = br#"{"jsonrpc":"2.0","id":"abc","method":"m","params":null}"#;
    let req: JsonRpcRequest<serde_json::Value> = parse_request(raw).unwrap();
    assert_eq!(req.id, Some(JsonRpcId::String("abc".into())));
}
