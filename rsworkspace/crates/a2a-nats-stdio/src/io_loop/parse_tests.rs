use super::*;

#[test]
fn parse_inbound_routes_syntax_to_parse_error_and_shape_to_invalid_request() {
    assert_eq!(parse_inbound("not json").unwrap_err().error.code, -32700);
    assert_eq!(parse_inbound(r#"{"id":1}"#).unwrap_err().error.code, -32600);
    let (id, method, _) = parse_inbound(r#"{"jsonrpc":"2.0","id":7,"method":"tasks/get","params":{}}"#).unwrap();
    assert_eq!(id, RpcId::Number(7));
    assert_eq!(method, "tasks/get");
}

#[test]
fn parse_inbound_preserves_id_on_envelope_failure() {
    let err = parse_inbound(r#"{"jsonrpc":"2.0","id":42}"#).unwrap_err();
    assert_eq!(err.id, RpcId::Number(42));
    let err = parse_inbound(r#"{"jsonrpc":"2.0","id":"corr-7"}"#).unwrap_err();
    assert_eq!(err.id, RpcId::String("corr-7".into()));
    let err = parse_inbound(r#"{"jsonrpc":"2.0"}"#).unwrap_err();
    assert_eq!(err.id, RpcId::Null);
    let err = parse_inbound(r#"{"jsonrpc":"2.0","id":[1,2,3]}"#).unwrap_err();
    assert_eq!(err.id, RpcId::Null);
}

#[test]
fn parse_inbound_rejects_missing_or_wrong_jsonrpc_version() {
    let err = parse_inbound(r#"{"id":1,"method":"tasks/get","params":{}}"#).unwrap_err();
    assert_eq!(err.error.code, -32600);
    assert_eq!(err.id, RpcId::Number(1));
    let err = parse_inbound(r#"{"jsonrpc":"1.0","id":2,"method":"tasks/get","params":{}}"#).unwrap_err();
    assert_eq!(err.error.code, -32600);
    assert_eq!(err.id, RpcId::Number(2));
    let err = parse_inbound(r#"{"jsonrpc":2.0,"id":3}"#).unwrap_err();
    assert_eq!(err.error.code, -32600);
}
