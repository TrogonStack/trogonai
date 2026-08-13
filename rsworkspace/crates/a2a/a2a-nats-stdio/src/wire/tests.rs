use super::*;
use bytes::Bytes;
use serde_json::json;

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
    let frame = OutboundFrame::error(ResponseId::Number(2), -32001, "not found");
    let v = serde_json::to_value(&frame).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["id"], 2);
    assert_eq!(v["error"]["code"], -32001);
    assert_eq!(v["error"]["message"], "not found");
}

#[test]
fn outbound_success_serializes() {
    let frame = OutboundFrame::success(ResponseId::Number(3), json!({"event": "x"}));
    let v = serde_json::to_value(&frame).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["id"], 3);
    assert_eq!(v["result"]["event"], "x");
    assert!(v.get("method").is_none(), "a response carries no method");
}

#[test]
fn null_id_still_serializes_as_a_response() {
    let frame = OutboundFrame::error(ResponseId::Null, -32600, "invalid");
    let v = serde_json::to_value(&frame).unwrap();
    assert_eq!(v["id"], Value::Null);
    assert_eq!(v["error"]["code"], -32600);
}

#[test]
fn with_error_id_stamps_errors_and_leaves_other_frames_alone() {
    let stamped = OutboundFrame::error(ResponseId::Null, -32602, "bad params").with_error_id(ResponseId::Number(9));
    assert_eq!(serde_json::to_value(&stamped).unwrap()["id"], 9);

    let raw = Bytes::from(serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"keep","result":{}})).unwrap());
    let untouched = OutboundFrame::RawBody(raw).with_error_id(ResponseId::Number(9));
    assert_eq!(serde_json::to_value(&untouched).unwrap()["id"], "keep");
}
