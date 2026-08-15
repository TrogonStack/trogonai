use super::*;
use jsonrpc_nats::from_json_value;

fn inbound(raw: &str) -> InboundRequest {
    serde_json::from_str(raw).expect("inbound request parses")
}

#[test]
fn success_and_error_envelopes_decode_as_canonical_jsonrpc() {
    let id = ResponseId::String("abc".to_string());
    let ok = success(&id, &serde_json::json!({ "taskId": "t1" })).expect("result serializes");
    let err = error(&id, jsonrpc_nats::INVALID_PARAMS, "invalid params");

    assert!(from_json_value(&ok).is_ok(), "{ok}");
    assert!(from_json_value(&err).is_ok(), "{err}");
    assert_eq!(ok["jsonrpc"], JSONRPC_VERSION);
    assert_eq!(err["jsonrpc"], JSONRPC_VERSION);
    assert_eq!(err["error"]["code"], jsonrpc_nats::INVALID_PARAMS);
}

#[test]
fn non_canonical_request_id_answers_with_null_rather_than_an_undecodable_envelope() {
    // A float id is neither a JSON-RPC string nor integer id. Echoing it back
    // would produce a response body the shared codec rejects, so the edge
    // answers with the spec's `null`.
    let request = inbound(r#"{"jsonrpc":"2.0","id":1.5,"method":"tasks/get"}"#);
    assert_eq!(request.response_id(), ResponseId::Null);

    let envelope = error(&request.response_id(), jsonrpc_nats::INVALID_REQUEST, "nope");
    assert_eq!(envelope["id"], Value::Null);
    assert!(from_json_value(&envelope).is_ok(), "{envelope}");
}

#[test]
fn canonical_request_ids_round_trip_onto_the_response() {
    assert_eq!(
        inbound(r#"{"jsonrpc":"2.0","id":"my-string-id","method":"tasks/get"}"#).response_id(),
        ResponseId::String("my-string-id".to_string())
    );
    assert_eq!(
        inbound(r#"{"jsonrpc":"2.0","id":7,"method":"tasks/get"}"#).response_id(),
        ResponseId::Number(7)
    );
    assert_eq!(
        inbound(r#"{"jsonrpc":"2.0","method":"tasks/get"}"#).response_id(),
        ResponseId::Null
    );
}

#[test]
fn version_gate_accepts_only_jsonrpc_2_0() {
    assert!(inbound(r#"{"jsonrpc":"2.0","id":1,"method":"tasks/get"}"#).has_supported_version());
    assert!(!inbound(r#"{"jsonrpc":"1.0","id":1,"method":"tasks/get"}"#).has_supported_version());
    assert!(!inbound(r#"{"id":1,"method":"tasks/get"}"#).has_supported_version());
    assert!(!inbound(r#"{"jsonrpc":2.0,"id":1,"method":"tasks/get"}"#).has_supported_version());
}

#[test]
fn malformed_members_parse_so_the_edge_can_answer_them_itself() {
    // Rejecting these at deserialization would hand the response back to the HTTP
    // layer, which has no id to correlate against and no JSON-RPC envelope to
    // put the failure in.
    assert_eq!(
        inbound(r#"{"jsonrpc":"2.0","id":1,"method":"tasks/get"}"#).method(),
        Some("tasks/get")
    );
    assert_eq!(inbound(r#"{"jsonrpc":"2.0","id":1}"#).method(), None);
    assert_eq!(inbound(r#"{"jsonrpc":"2.0","id":1,"method":42}"#).method(), None);
    assert_eq!(inbound(r#"{"jsonrpc":"2.0","id":1,"method":null}"#).method(), None);
    assert_eq!(
        inbound(r#"{"jsonrpc":2.0,"id":1,"method":"tasks/get"}"#).response_id(),
        ResponseId::Number(1)
    );
}

#[test]
fn omitted_params_read_as_null() {
    assert_eq!(
        inbound(r#"{"jsonrpc":"2.0","id":1,"method":"tasks/get"}"#).params(),
        Value::Null
    );
    assert_eq!(
        inbound(r#"{"jsonrpc":"2.0","id":1,"method":"tasks/get","params":{"id":"t1"}}"#).params(),
        serde_json::json!({ "id": "t1" })
    );
}

#[test]
fn rest_error_carries_the_error_member_without_an_envelope() {
    let body = serde_json::to_value(RestError::new(jsonrpc_nats::INVALID_PARAMS, "bad id")).expect("serializes");
    assert_eq!(body["error"]["code"], jsonrpc_nats::INVALID_PARAMS);
    assert_eq!(body["error"]["message"], "bad id");
    assert!(body.get("jsonrpc").is_none());
}
