use async_nats::HeaderMap;
use serde_json::json;

use super::*;

fn headers_with_req_id(req_id: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(a2a_nats::constants::REQ_ID_HEADER, req_id);
    h
}

fn payload_without_id() -> &'static [u8] {
    br#"{"jsonrpc":"2.0","method":"tasks/resubscribe","params":{}}"#
}

#[test]
fn task_id_resolves_from_id_field() {
    let params = json!({"id": "t-1"});
    assert_eq!(
        task_id_from_resubscribe_params(&params).map(|t| t.as_str().to_owned()),
        Some("t-1".to_owned()),
    );
}

#[test]
fn task_id_resolves_from_snake_case_alias() {
    let params = json!({"task_id": "t-2"});
    assert_eq!(
        task_id_from_resubscribe_params(&params).map(|t| t.as_str().to_owned()),
        Some("t-2".to_owned()),
    );
}

#[test]
fn task_id_resolves_from_camel_case_alias() {
    let params = json!({"taskId": "t-3"});
    assert_eq!(
        task_id_from_resubscribe_params(&params).map(|t| t.as_str().to_owned()),
        Some("t-3".to_owned()),
    );
}

#[test]
fn task_id_returns_none_when_field_missing() {
    let params = json!({"unrelated": "value"});
    assert!(task_id_from_resubscribe_params(&params).is_none());
}

#[test]
fn task_id_returns_none_when_value_is_not_string() {
    let params = json!({"id": 42});
    assert!(task_id_from_resubscribe_params(&params).is_none());
}

#[test]
fn task_id_returns_none_when_string_fails_validation() {
    // Empty / unsafe NATS tokens must not slip past as a valid task
    // id -- the pump would then bind a stream consumer on a
    // malformed subject.
    let params = json!({"id": ""});
    assert!(task_id_from_resubscribe_params(&params).is_none());
}

#[test]
fn last_seq_resolves_from_snake_and_camel_case() {
    let snake = json!({"last_seq": 17_u64});
    let camel = json!({"lastSeq": 23_u64});
    assert_eq!(last_seq_from_resubscribe_params(&snake), Some(17));
    assert_eq!(last_seq_from_resubscribe_params(&camel), Some(23));
}

#[test]
fn last_seq_returns_none_when_absent_or_non_integer() {
    // Absence means "no resume cursor" rather than "resume at 0";
    // the pump treats them differently. A non-integer value also
    // resolves to `None` so a malformed payload doesn't get
    // silently coerced.
    assert!(last_seq_from_resubscribe_params(&json!({})).is_none());
    assert!(last_seq_from_resubscribe_params(&json!({"last_seq": "not-a-number"})).is_none());
}

#[test]
fn classify_returns_not_streaming_for_unrelated_method() {
    let headers = headers_with_req_id("r-1");
    let intent = classify_streaming_spawn("message.send", &headers, b"");
    assert!(matches!(intent, StreamingSpawnIntent::NotStreaming));
}

#[test]
fn classify_returns_not_streaming_when_req_id_missing() {
    // A streaming method without a req id can't be correlated back
    // to the caller's inbox, so the wrapper falls through to the
    // unary error path rather than spawning a pump that nobody can
    // join.
    let headers = HeaderMap::new();
    let intent = classify_streaming_spawn("message.stream", &headers, payload_without_id());
    assert!(matches!(intent, StreamingSpawnIntent::NotStreaming));
}

#[test]
fn classify_spawns_message_stream_kind() {
    let headers = headers_with_req_id("r-2");
    let intent = classify_streaming_spawn("message.stream", &headers, b"");
    match intent {
        StreamingSpawnIntent::Spawn(StreamingIngressKind::MessageStream { .. }) => {}
        other => panic!("expected MessageStream kind, got {other:?}"),
    }
}

#[test]
fn classify_spawns_tasks_resubscribe_kind_with_resume_cursor() {
    let headers = headers_with_req_id("r-3");
    let payload = br#"{"jsonrpc":"2.0","id":"r-3","method":"tasks/resubscribe","params":{"id":"t-1","last_seq":5}}"#;
    let intent = classify_streaming_spawn("tasks.resubscribe", &headers, payload);
    match intent {
        StreamingSpawnIntent::Spawn(StreamingIngressKind::TasksResubscribe { task_id, last_seq, .. }) => {
            assert_eq!(task_id.as_str(), "t-1");
            assert_eq!(last_seq, 5);
        }
        other => panic!("expected TasksResubscribe kind, got {other:?}"),
    }
}

#[test]
fn classify_tasks_resubscribe_without_task_id_falls_through() {
    // A resubscribe request with no parseable task id must NOT
    // spawn the pump -- the unary error path will surface
    // -32600/invalid_request to the caller.
    let headers = headers_with_req_id("r-4");
    let payload = br#"{"jsonrpc":"2.0","id":"r-4","method":"tasks/resubscribe","params":{}}"#;
    let intent = classify_streaming_spawn("tasks.resubscribe", &headers, payload);
    assert!(matches!(intent, StreamingSpawnIntent::NotStreaming));
}

#[test]
fn classify_tasks_resubscribe_defaults_last_seq_to_zero_when_absent() {
    let headers = headers_with_req_id("r-5");
    let payload = br#"{"jsonrpc":"2.0","id":"r-5","method":"tasks/resubscribe","params":{"id":"t-1"}}"#;
    let intent = classify_streaming_spawn("tasks.resubscribe", &headers, payload);
    match intent {
        StreamingSpawnIntent::Spawn(StreamingIngressKind::TasksResubscribe { last_seq, .. }) => {
            assert_eq!(last_seq, 0);
        }
        other => panic!("expected TasksResubscribe kind with default last_seq=0, got {other:?}"),
    }
}
