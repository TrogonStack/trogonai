use super::*;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct DummyParams {
    value: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct DummyResult {
    answer: i32,
}

#[test]
fn jsonrpc_request_serializes_with_version_and_method() {
    let req = JsonRpcRequest::new(JsonRpcId::Number(1), "tasks/get", DummyParams { value: "x".into() });
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["method"], "tasks/get");
    assert_eq!(v["id"], 1);
}

#[test]
fn jsonrpc_response_deserializes_success() {
    let json = r#"{"jsonrpc":"2.0","id":1,"result":{"answer":42}}"#;
    let resp: JsonRpcResponse<DummyResult> = serde_json::from_str(json).unwrap();
    assert!(matches!(resp, JsonRpcResponse::Success(s) if s.result.answer == 42));
}

#[test]
fn jsonrpc_response_deserializes_error() {
    let json = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32001,"message":"not found"}}"#;
    let resp: JsonRpcResponse<DummyResult> = serde_json::from_str(json).unwrap();
    assert!(matches!(resp, JsonRpcResponse::Error(e) if e.error.code == -32001));
}

#[test]
fn jsonrpc_error_body_captures_message() {
    let json = r#"{"code":-32050,"message":"agent down"}"#;
    let body: JsonRpcErrorBody = serde_json::from_str(json).unwrap();
    assert_eq!(body.code, -32050);
    assert_eq!(body.message, "agent down");
}

#[test]
fn jsonrpc_request_string_id() {
    let req = JsonRpcRequest::new(
        JsonRpcId::String("abc".into()),
        "message/send",
        DummyParams { value: "y".into() },
    );
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(v["id"], "abc");
}
