use super::*;

fn applied(plan: AckPlan, payload: &Value) -> HeaderMap {
    let mut headers = HeaderMap::new();
    RequestHeaders::from_payload(plan, payload).unwrap().apply(&mut headers);
    headers
}

fn value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).map(|value| value.as_str())
}

#[test]
fn a_modal_submission_names_its_callback() {
    let headers = applied(
        AckPlan::AwaitReply,
        &serde_json::json!({ "view": { "callback_id": "my_modal" }, "action_id": "ignored" }),
    );
    assert_eq!(value(&headers, NATS_HEADER_CALLBACK_ID), Some("my_modal"));
    assert_eq!(value(&headers, NATS_HEADER_ACTION_ID), None);
}

#[test]
fn an_options_request_names_its_action_and_block() {
    let headers = applied(
        AckPlan::RequestOnly,
        &serde_json::json!({ "action_id": "pick", "block_id": "blk", "view": { "callback_id": "ignored" } }),
    );
    assert_eq!(value(&headers, NATS_HEADER_ACTION_ID), Some("pick"));
    assert_eq!(value(&headers, NATS_HEADER_BLOCK_ID), Some("blk"));
    assert_eq!(value(&headers, NATS_HEADER_CALLBACK_ID), None);
}

#[test]
fn identifiers_that_are_absent_empty_or_not_text_add_nothing() {
    for payload in [
        serde_json::json!({}),
        serde_json::json!({ "view": { "callback_id": "" } }),
        serde_json::json!({ "view": { "callback_id": 7 } }),
    ] {
        assert!(applied(AckPlan::AwaitReply, &payload).is_empty());
    }
}

#[test]
fn an_acknowledged_interaction_adds_nothing() {
    let payload = serde_json::json!({ "view": { "callback_id": "m" }, "action_id": "a", "block_id": "b" });
    assert!(applied(AckPlan::Acknowledge, &payload).is_empty());
}

#[test]
fn an_identifier_that_cannot_be_header_text_is_refused() {
    let payload = serde_json::json!({ "action_id": "bad\nid" });
    assert!(RequestHeaders::from_payload(AckPlan::RequestOnly, &payload).is_err());
}
