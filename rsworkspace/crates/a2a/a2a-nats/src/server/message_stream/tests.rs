use futures::stream;
use jsonrpc_nats::RequestId;
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::mocks::MockJetStreamPublisher;

use super::*;
use crate::server::test_support::{parse_published_response, stub, wire_notification, wire_request};

fn prefix() -> A2aPrefix {
    A2aPrefix::new("a2a").unwrap()
}

fn stream_params() -> serde_json::Value {
    serde_json::json!({
        "message": {
            "messageId": "m-1",
            "role": "ROLE_USER",
            "parts": []
        }
    })
}

fn stream_payload(req_id: &str) -> (async_nats::HeaderMap, Vec<u8>) {
    wire_request("message/stream", RequestId::String(req_id.to_string()), stream_params())
}

fn stream_payload_numeric_id(req_id: i64) -> (async_nats::HeaderMap, Vec<u8>) {
    wire_request("message/stream", RequestId::Number(req_id), stream_params())
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

    let (headers, payload) = stream_payload("call-1");
    handle(&handler, &headers, &payload, Some("r".into()), &nats, &js, &prefix()).await;

    let body = parse_published_response(&nats, 0);
    assert_eq!(body["result"]["task"]["id"].as_str(), Some("task-1"));
    assert_eq!(js.published_subjects(), vec!["a2a.v1.tasks.task-1.events".to_string()]);
}

#[tokio::test]
async fn every_event_carries_the_request_id_header() {
    // The subject names only the task, so `Trogon-Req-Id` is what tells two
    // concurrent subscriptions to the same task apart.
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let events: crate::server::handler::TaskEventStream = Box::pin(stream::iter(vec![
        Ok(working_status_event("task-1")),
        Ok(working_status_event("task-1")),
    ]));
    handler.lock().unwrap().message_stream_result = Some(Ok((task("task-1"), events)));

    let (headers, payload) = stream_payload("call-1");
    handle(&handler, &headers, &payload, Some("r".into()), &nats, &js, &prefix()).await;

    let published = js.published_messages();
    assert_eq!(published.len(), 2);
    for message in published {
        assert_eq!(
            message.headers.get(crate::constants::REQ_ID_HEADER).map(|v| v.as_str()),
            Some("call-1")
        );
    }
}

#[tokio::test]
async fn numeric_jsonrpc_id_rejected_with_invalid_params() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let (headers, payload) = stream_payload_numeric_id(42);
    handle(&handler, &headers, &payload, Some("r".into()), &nats, &js, &prefix()).await;
    let body = parse_published_response(&nats, 0);
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

    let (headers, payload) = stream_payload("call-2");
    handle(&handler, &headers, &payload, Some("r".into()), &nats, &js, &prefix()).await;

    let body = parse_published_response(&nats, 0);
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
    let (headers, payload) = stream_payload("call-3");
    handle(&handler, &headers, &payload, None, &nats, &js, &prefix()).await;
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn missing_params_returns_invalid_params_error() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let (headers, payload) = wire_request(
        "message/stream",
        RequestId::String("call-4".into()),
        serde_json::Value::Null,
    );
    handle(&handler, &headers, &payload, Some("r".into()), &nats, &js, &prefix()).await;
    let body = parse_published_response(&nats, 0);
    assert_eq!(body["error"]["code"], -32602);
}

#[tokio::test]
async fn invalid_params_shape_returns_invalid_params_code() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let (headers, payload) = wire_request(
        "message/stream",
        RequestId::String("call-5".into()),
        serde_json::json!({"message": 42}),
    );
    handle(&handler, &headers, &payload, Some("r".into()), &nats, &js, &prefix()).await;
    let body = parse_published_response(&nats, 0);
    assert_eq!(body["error"]["code"], -32602);
    assert_eq!(body["id"], "call-5");
}

#[tokio::test]
async fn malformed_json_still_publishes_parse_error_with_null_id() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(jsonrpc_nats::HEADER_ID, "null");
    handle(&handler, &headers, b"not json", Some("r".into()), &nats, &js, &prefix()).await;
    let body = parse_published_response(&nats, 0);
    assert_eq!(body["error"]["code"], -32700);
    assert!(body["id"].is_null());
}

#[tokio::test]
async fn notification_without_id_is_dropped() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let (headers, payload) = wire_notification("message/stream", stream_params());
    handle(&handler, &headers, &payload, None, &nats, &js, &prefix()).await;
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn invalid_task_id_from_handler_returns_invalid_agent_response() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let events: crate::server::handler::TaskEventStream = Box::pin(stream::iter(vec![]));
    handler.lock().unwrap().message_stream_result = Some(Ok((task(""), events)));

    let (headers, payload) = stream_payload("call-8");
    handle(&handler, &headers, &payload, Some("r".into()), &nats, &js, &prefix()).await;

    let body = parse_published_response(&nats, 0);
    assert_eq!(
        body["error"]["code"].as_i64(),
        Some(i64::from(crate::error::INVALID_AGENT_RESPONSE))
    );
}

#[tokio::test]
async fn events_carry_the_callers_identity_forward_from_the_request() {
    // The gateway's egress pump gates per caller and can no longer recover the
    // caller from the subject, so identity has to ride the event headers.
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let events: crate::server::handler::TaskEventStream =
        Box::pin(stream::iter(vec![Ok(working_status_event("task-1"))]));
    handler.lock().unwrap().message_stream_result = Some(Ok((task("task-1"), events)));

    let (mut headers, payload) = stream_payload("call-1");
    headers.insert(crate::constants::GATEWAY_CALLER_ID_HEADER, "bot");
    headers.insert(crate::constants::GATEWAY_PRINCIPAL_HEADER, "principal");
    handle(&handler, &headers, &payload, Some("r".into()), &nats, &js, &prefix()).await;

    let published = js.published_messages();
    let event = published.first().expect("one event");
    assert_eq!(
        event
            .headers
            .get(crate::constants::GATEWAY_CALLER_ID_HEADER)
            .map(|v| v.as_str()),
        Some("bot")
    );
    assert_eq!(
        event
            .headers
            .get(crate::constants::GATEWAY_PRINCIPAL_HEADER)
            .map(|v| v.as_str()),
        Some("principal")
    );
}
