use super::*;
use bytes::Bytes;
use serde_json::json;

#[test]
fn inbound_request_deserializes_numeric_id() {
    let raw = r#"{"jsonrpc":"2.0","id":42,"method":"tasks/get","params":{"id":"t1","tenant":""}}"#;
    let req: InboundRequest = serde_json::from_str(raw).unwrap();
    assert_eq!(req.id, RpcId::Number(42));
    assert_eq!(req.method, "tasks/get");
}

#[test]
fn inbound_request_deserializes_string_id() {
    let raw = r#"{"jsonrpc":"2.0","id":"abc","method":"agent/getAuthenticatedExtendedCard","params":{}}"#;
    let req: InboundRequest = serde_json::from_str(raw).unwrap();
    assert_eq!(req.id, RpcId::String("abc".into()));
}

#[test]
fn outbound_raw_body_rewrites_via_serde() {
    let body =
        Bytes::from(serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"transport","result":{"id":"task-1"}})).unwrap());
    let frame = OutboundFrame::RawBody(body);
    let v = serde_json::to_value(&frame).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["id"], "transport");
    assert_eq!(v["result"]["id"], "task-1");
}

#[test]
fn outbound_error_serializes() {
    let err = OutboundError::new(RpcId::Number(2), -32001, "not found".into());
    let v = serde_json::to_value(&err).unwrap();
    assert_eq!(v["error"]["code"], -32001);
    assert_eq!(v["error"]["message"], "not found");
}

#[test]
fn outbound_notification_serializes() {
    let notif = OutboundNotification::new(RpcId::Number(3), "message/stream", json!({"event": "x"}));
    let v = serde_json::to_value(&notif).unwrap();
    assert_eq!(v["method"], "message/stream");
    assert_eq!(v["id"], 3);
}

#[test]
fn outbound_frame_error_variant_serializes() {
    let frame = OutboundFrame::Error(OutboundError::new(RpcId::Null, -32600, "invalid".into()));
    let v = serde_json::to_value(&frame).unwrap();
    assert_eq!(v["error"]["code"], -32600);
}

#[test]
fn rpc_id_projects_every_json_rpc_id_shape() {
    assert_eq!(RpcId::Number(7).to_json_value(), json!(7));
    assert_eq!(RpcId::String("abc".into()).to_json_value(), json!("abc"));
    assert_eq!(RpcId::Null.to_json_value(), Value::Null);
}
