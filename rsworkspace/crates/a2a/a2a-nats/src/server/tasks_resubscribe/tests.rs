use jsonrpc_nats::RequestId;
use trogon_nats::AdvancedMockNatsClient;

use super::*;
use crate::server::test_support::{parse_published_response, stub, wire_notification, wire_request};

fn resubscribe_payload(id: i64, task_id: &str) -> (async_nats::HeaderMap, Vec<u8>) {
    wire_request(
        "tasks/resubscribe",
        RequestId::Number(id),
        serde_json::json!({ "id": task_id }),
    )
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

#[tokio::test]
async fn success_publishes_snapshot() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    handler.lock().unwrap().tasks_resubscribe_result = Some(Ok(task("t-1")));
    let (headers, payload) = resubscribe_payload(1, "t-1");
    handle(&handler, &headers, &payload, Some("r".into()), &nats).await;
    let body = parse_published_response(&nats, 0);
    assert_eq!(body["result"]["id"].as_str(), Some("t-1"));
}

#[tokio::test]
async fn task_not_found_error_uses_typed_code() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    handler.lock().unwrap().tasks_resubscribe_result = Some(Err(A2aError::task_not_found("missing")));
    let (headers, payload) = resubscribe_payload(2, "t");
    handle(&handler, &headers, &payload, Some("r".into()), &nats).await;
    let body = parse_published_response(&nats, 0);
    assert_eq!(
        body["error"]["code"].as_i64(),
        Some(i64::from(crate::error::TASK_NOT_FOUND))
    );
}

#[tokio::test]
async fn no_reply_drops_request() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    let (headers, payload) = resubscribe_payload(3, "t");
    handle(&handler, &headers, &payload, None, &nats).await;
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn missing_params_returns_invalid_params_error() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    let (headers, payload) = wire_request("METHOD", RequestId::Number(5), serde_json::Value::Null);
    handle(&handler, &headers, &payload, Some("r".into()), &nats).await;
    let body = parse_published_response(&nats, 0);
    assert_eq!(body["error"]["code"], -32602);
}

#[tokio::test]
async fn invalid_params_shape_returns_invalid_params_code() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    let (headers, payload) = wire_request(
        "tasks/resubscribe",
        RequestId::Number(6),
        serde_json::json!({ "id": 42 }),
    );
    handle(&handler, &headers, &payload, Some("r".into()), &nats).await;
    let body = parse_published_response(&nats, 0);
    assert_eq!(body["error"]["code"], -32602);
    assert_eq!(body["id"], 6);
}

#[tokio::test]
async fn malformed_json_still_publishes_parse_error_with_null_id() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(jsonrpc_nats::HEADER_ID, "null");
    handle(&handler, &headers, b"not json", Some("r".into()), &nats).await;
    let body = parse_published_response(&nats, 0);
    assert_eq!(body["error"]["code"], -32700);
    assert!(body["id"].is_null());
}

#[tokio::test]
async fn notification_without_id_is_dropped() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    let (headers, payload) = wire_notification("tasks/resubscribe", serde_json::json!({ "id": "t" }));
    handle(&handler, &headers, &payload, Some("r".into()), &nats).await;
    assert!(nats.published_messages().is_empty());
}
