use super::*;
use serde_json::{Value, json};

/// The id the bridge echoes back on a rejected line, as it lands on stdout.
#[track_caller]
fn err_id(raw: &str) -> Value {
    serde_json::to_value(*parse_inbound(raw).unwrap_err()).unwrap()["id"].clone()
}

#[test]
fn parse_inbound_routes_syntax_to_parse_error_and_shape_to_invalid_request() {
    assert_eq!(parse_inbound("not json").unwrap_err().error_code().unwrap(), -32700);
    assert_eq!(parse_inbound(r#"{"id":1}"#).unwrap_err().error_code().unwrap(), -32600);
    let (id, method, _) = parse_inbound(r#"{"jsonrpc":"2.0","id":7,"method":"tasks/get","params":{}}"#).unwrap();
    assert_eq!(id, RequestId::Number(7));
    assert_eq!(method, "tasks/get");
}

#[test]
fn parse_inbound_preserves_id_on_envelope_failure() {
    assert_eq!(err_id(r#"{"jsonrpc":"2.0","id":42}"#), json!(42));
    assert_eq!(err_id(r#"{"jsonrpc":"2.0","id":"corr-7"}"#), json!("corr-7"));
    assert_eq!(err_id(r#"{"jsonrpc":"2.0"}"#), Value::Null);
    assert_eq!(err_id(r#"{"jsonrpc":"2.0","id":[1,2,3]}"#), Value::Null);
}

#[test]
fn parse_inbound_rejects_missing_or_wrong_jsonrpc_version() {
    let err = parse_inbound(r#"{"id":1,"method":"tasks/get","params":{}}"#).unwrap_err();
    assert_eq!(err.error_code().unwrap(), -32600);
    assert_eq!(err_id(r#"{"id":1,"method":"tasks/get","params":{}}"#), json!(1));
    let err = parse_inbound(r#"{"jsonrpc":"1.0","id":2,"method":"tasks/get","params":{}}"#).unwrap_err();
    assert_eq!(err.error_code().unwrap(), -32600);
    assert_eq!(
        err_id(r#"{"jsonrpc":"1.0","id":2,"method":"tasks/get","params":{}}"#),
        json!(2)
    );
    let err = parse_inbound(r#"{"jsonrpc":2.0,"id":3}"#).unwrap_err();
    assert_eq!(err.error_code().unwrap(), -32600);
}

#[test]
fn parse_inbound_rejects_a_notification() {
    // A notification carries no id, so the stdio bridge has nothing to
    // correlate a reply with; it answers requests only.
    let err = parse_inbound(r#"{"jsonrpc":"2.0","method":"tasks/get","params":{}}"#).unwrap_err();
    assert_eq!(err.error_code().unwrap(), -32600);
    assert_eq!(serde_json::to_value(*err).unwrap()["id"], Value::Null);
}
