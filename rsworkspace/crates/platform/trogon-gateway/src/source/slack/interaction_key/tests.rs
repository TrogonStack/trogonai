use super::*;
use serde_json::json;

fn key(payload: serde_json::Value, interaction_type: &str) -> InteractionKey {
    let body = payload.to_string();
    InteractionKey::derive(&payload, interaction_type, body.as_bytes())
}

#[test]
fn trigger_id_is_the_key_when_slack_sends_one() {
    let key = key(json!({ "type": "block_actions", "trigger_id": "t-1" }), "block_actions");
    assert_eq!(key.as_str(), "t-1");
}

#[test]
fn view_closed_is_keyed_by_the_view_it_closed() {
    let key = key(json!({ "type": "view_closed", "view": { "id": "V01" } }), "view_closed");
    assert_eq!(key.as_str(), "view_closed-V01");
}

#[test]
fn other_interactions_without_a_trigger_are_keyed_by_their_exact_bytes() {
    let first = key(json!({ "type": "block_suggestion", "value": "ab" }), "block_suggestion");
    let same = key(json!({ "type": "block_suggestion", "value": "ab" }), "block_suggestion");
    let other = key(
        json!({ "type": "block_suggestion", "value": "abc" }),
        "block_suggestion",
    );
    assert_eq!(first, same);
    assert_ne!(first, other);
    assert!(first.as_str().starts_with("interaction-"));
}

#[test]
fn empty_or_non_string_trigger_ids_are_treated_as_absent() {
    for trigger in [json!(""), json!(7), json!(null)] {
        let key = key(
            json!({ "type": "block_suggestion", "trigger_id": trigger }),
            "block_suggestion",
        );
        assert!(key.as_str().starts_with("interaction-"));
    }
}

#[test]
fn view_closed_without_a_view_id_falls_back_to_the_digest() {
    let key = key(json!({ "type": "view_closed", "view": {} }), "view_closed");
    assert!(key.as_str().starts_with("interaction-"));
}
