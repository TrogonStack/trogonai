use futures::stream;
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::mocks::MockJetStreamPublisher;

use super::*;
use crate::server::test_support::{parse_response, stub};

fn prefix() -> A2aPrefix {
    A2aPrefix::new("a2a").unwrap()
}

fn stream_payload(req_id: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": req_id,
        "method": "message/stream",
        "params": {
            "message": {
                "messageId": "m-1",
                "role": "ROLE_USER",
                "parts": []
            }
        }
    }))
    .unwrap()
}

fn stream_payload_numeric_id(req_id: i64) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": req_id,
        "method": "message/stream",
        "params": {
            "message": {
                "messageId": "m-1",
                "role": "ROLE_USER",
                "parts": []
            }
        }
    }))
    .unwrap()
}

fn task(task_id: &str) -> a2a::types::Task {
    a2a::types::Task {
        id: task_id.to_string(),
        context_id: String::new(),
        status: a2a::types::TaskStatus {
            state: a2a::types::TaskState::Working,
            message: None,
            timestamp: None,
        },
        artifacts: None,
        history: None,
        metadata: None,
    }
}

fn working_status_event(task_id: &str) -> a2a::event::StreamResponse {
    a2a::event::StreamResponse::StatusUpdate(a2a::event::TaskStatusUpdateEvent {
        task_id: task_id.to_string(),
        context_id: "ctx".to_string(),
        status: a2a::types::TaskStatus {
            state: a2a::types::TaskState::Working,
            message: None,
            timestamp: None,
        },
        metadata: None,
    })
}

#[tokio::test]
async fn bootstrap_publishes_task_and_then_events() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let events: crate::server::handler::TaskEventStream =
        Box::pin(stream::iter(vec![Ok(working_status_event("task-1"))]));
    handler.lock().unwrap().message_stream_result = Some(Ok((task("task-1"), events)));

    handle(
        &handler,
        &stream_payload("call-1"),
        Some("r".into()),
        &nats,
        &js,
        &prefix(),
    )
    .await;

    let body = parse_response(&nats.published_payloads()[0]);
    assert_eq!(body["result"]["task"]["id"].as_str(), Some("task-1"));
    let subjects = js.published_subjects();
    // The event subject MUST use the caller's JSON-RPC id as the suffix so it
    // matches `stream_events_consumer`'s `{prefix}.tasks.*.events.{req_id}` filter
    // on the client side; otherwise streamed events never reach the subscriber.
    assert_eq!(subjects, vec!["a2a.tasks.task-1.events.call-1".to_string()]);
}

#[tokio::test]
async fn numeric_jsonrpc_id_rejected_with_invalid_params() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    // Handler should not even be called; if it were, this would yield a panic on
    // the unwrap-stub default. We just want to confirm the error reply shape.
    handle(
        &handler,
        &stream_payload_numeric_id(42),
        Some("r".into()),
        &nats,
        &js,
        &prefix(),
    )
    .await;
    let body = parse_response(&nats.published_payloads()[0]);
    assert_eq!(body["error"]["code"], -32602);
    assert_eq!(body["id"], 42);
    assert!(js.published_subjects().is_empty());
}

#[tokio::test]
async fn handler_error_published_as_bootstrap_error() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    handler.lock().unwrap().message_stream_result = Some(Err(A2aError::agent_unavailable("down")));

    handle(
        &handler,
        &stream_payload("call-2"),
        Some("r".into()),
        &nats,
        &js,
        &prefix(),
    )
    .await;

    let body = parse_response(&nats.published_payloads()[0]);
    assert_eq!(
        body["error"]["code"].as_i64(),
        Some(i64::from(crate::error::AGENT_UNAVAILABLE))
    );
    assert!(js.published_subjects().is_empty());
}

#[tokio::test]
async fn no_reply_drops_request() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    handle(&handler, &stream_payload("call-3"), None, &nats, &js, &prefix()).await;
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn missing_params_returns_invalid_params_error() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let payload = serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "message/stream"
    }))
    .unwrap();
    handle(&handler, &payload, Some("r".into()), &nats, &js, &prefix()).await;
    let body = parse_response(&nats.published_payloads()[0]);
    assert_eq!(body["error"]["code"], -32602);
}

#[tokio::test]
async fn invalid_params_shape_returns_invalid_params_code() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let payload = serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "message/stream",
        "params": {"message": 42}
    }))
    .unwrap();
    handle(&handler, &payload, Some("r".into()), &nats, &js, &prefix()).await;
    let body = parse_response(&nats.published_payloads()[0]);
    assert_eq!(body["error"]["code"], -32602);
    assert_eq!(body["id"], 6);
}

#[tokio::test]
async fn malformed_json_still_publishes_parse_error_with_null_id() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    handle(&handler, b"not json", Some("r".into()), &nats, &js, &prefix()).await;
    let body = parse_response(&nats.published_payloads()[0]);
    assert_eq!(body["error"]["code"], -32700);
    assert!(body["id"].is_null());
}

#[tokio::test]
async fn notification_without_id_is_dropped() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let payload = serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "message/stream",
        "params": {"message": {"messageId":"m","role":"ROLE_USER","parts":[]}}
    }))
    .unwrap();
    handle(&handler, &payload, Some("r".into()), &nats, &js, &prefix()).await;
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn publish_returns_false_when_serialize_fails() {
    let nats = AdvancedMockNatsClient::new();
    let err: Result<bytes::Bytes, serde_json::Error> = Err(serde_json::from_str::<String>("x").unwrap_err());
    let ok = publish(&nats, "r", err, "test-label").await;
    assert!(!ok);
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn publish_returns_false_when_nats_publish_fails() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let ok = publish(&nats, "r", Ok(bytes::Bytes::from_static(b"{}")), "test-label").await;
    assert!(!ok);
}

#[tokio::test]
async fn invalid_task_id_from_handler_returns_invalid_agent_response() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let events: crate::server::handler::TaskEventStream = Box::pin(stream::iter(vec![]));
    handler.lock().unwrap().message_stream_result = Some(Ok((task(""), events)));

    handle(
        &handler,
        &stream_payload("call-8"),
        Some("r".into()),
        &nats,
        &js,
        &prefix(),
    )
    .await;

    let body = parse_response(&nats.published_payloads()[0]);
    assert_eq!(
        body["error"]["code"].as_i64(),
        Some(i64::from(crate::error::INVALID_AGENT_RESPONSE))
    );
}
