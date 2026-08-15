use a2a::event::TaskStatusUpdateEvent;
use a2a::types::{TaskState, TaskStatus};

use super::*;

fn status_event(task_id: &str) -> StreamResponse {
    StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
        task_id: task_id.to_string(),
        context_id: "ctx".to_string(),
        status: TaskStatus {
            state: TaskState::Working,
            message: None,
            timestamp: None,
        },
        metadata: None,
    })
}

/// What the previous release published: the event itself, no envelope.
fn legacy_body(task_id: &str) -> Vec<u8> {
    serde_json::to_vec(&status_event(task_id)).unwrap()
}

#[test]
fn a_pre_envelope_event_still_decodes() {
    let decoded = decode_legacy_event(&legacy_body("task-1")).expect("an older agent's event is still readable");
    assert_eq!(decoded, status_event("task-1"));
}

#[test]
fn a_current_envelope_is_not_mistaken_for_a_legacy_event() {
    // Both shapes reach a reader during a rolling upgrade, so the two must stay
    // distinguishable: an envelope decoded as a legacy event would lose its id and
    // hide a JSON-RPC error as a success.
    let body = serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": "req-1",
        "result": status_event("task-1"),
    }))
    .unwrap();
    assert!(decode_legacy_event(&body).is_none());
}

#[test]
fn a_malformed_body_is_not_dressed_up_as_an_event() {
    assert!(decode_legacy_event(b"not json").is_none());
    assert!(decode_legacy_event(br#"{"unknownVariant":{}}"#).is_none());
}

#[test]
fn a_legacy_event_lifts_into_a_response_carrying_the_callers_id() {
    let response = legacy_event_as_response(&legacy_body("task-1"), &Value::String("corr-1".to_owned()))
        .expect("a legacy event is liftable");

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], "corr-1");
    assert_eq!(
        serde_json::from_value::<StreamResponse>(response["result"].clone()).unwrap(),
        status_event("task-1")
    );
}

#[test]
fn nothing_else_lifts() {
    assert!(legacy_event_as_response(b"not json", &Value::Null).is_none());
}
