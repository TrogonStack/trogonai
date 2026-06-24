use jsonrpc_nats::RequestId;
use trogon_nats::AdvancedMockNatsClient;

use super::*;
use crate::server::test_support::{parse_published_response, rpc_payload, stub, wire_notification, wire_request};

fn send_message_request_payload(id: i64) -> (async_nats::HeaderMap, Vec<u8>) {
    wire_request(
        "message/send",
        RequestId::Number(id),
        serde_json::json!({
            "message": {
                "messageId": "m-1",
                "role": "ROLE_USER",
                "parts": []
            }
        }),
    )
}

fn task_response(task_id: &str) -> a2a::types::SendMessageResponse {
    a2a::types::SendMessageResponse::Task(a2a::types::Task {
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
    })
}

#[tokio::test]
async fn success_publishes_task_response() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    handler.lock().unwrap().message_send_result = Some(Ok(task_response("t-1")));
    let (headers, payload) = send_message_request_payload(1);
    handle(&handler, &headers, &payload, Some("r".into()), &nats).await;
    let body = parse_published_response(&nats, 0);
    assert_eq!(body["result"]["task"]["id"].as_str(), Some("t-1"));
}

#[tokio::test]
async fn handler_error_response_uses_typed_code() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    handler.lock().unwrap().message_send_result = Some(Err(A2aError::agent_unavailable("down")));
    let (headers, payload) = send_message_request_payload(2);
    handle(&handler, &headers, &payload, Some("r".into()), &nats).await;
    let body = parse_published_response(&nats, 0);
    assert_eq!(
        body["error"]["code"].as_i64(),
        Some(i64::from(crate::error::AGENT_UNAVAILABLE))
    );
}

#[tokio::test]
async fn no_reply_drops_request() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    let (headers, payload) = send_message_request_payload(3);
    handle(&handler, &headers, &payload, None, &nats).await;
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn missing_params_returns_invalid_params_error() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    let (headers, payload) = rpc_payload("message/send", 4);
    handle(&handler, &headers, &payload, Some("r".into()), &nats).await;

    let (headers, payload) = wire_request("message/send", RequestId::Number(5), serde_json::Value::Null);
    let nats = AdvancedMockNatsClient::new();
    handle(&handler, &headers, &payload, Some("r".into()), &nats).await;
    let body = parse_published_response(&nats, 0);
    assert_eq!(body["error"]["code"], -32602);
}

#[tokio::test]
async fn invalid_params_shape_returns_invalid_params_code() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    let (headers, payload) = wire_request(
        "message/send",
        RequestId::Number(6),
        serde_json::json!({"message": 42}),
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
    handle(
        &handler,
        &async_nats::HeaderMap::new(),
        b"not json",
        Some("r".into()),
        &nats,
    )
    .await;
    let body = parse_published_response(&nats, 0);
    assert_eq!(body["error"]["code"], -32700);
    assert!(body["id"].is_null());
}

#[tokio::test]
async fn notification_without_id_is_dropped() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    let (headers, payload) = wire_notification(
        "message/send",
        serde_json::json!({"message": {"messageId":"m","role":"user","parts":[]}}),
    );
    handle(&handler, &headers, &payload, Some("r".into()), &nats).await;
    assert!(nats.published_messages().is_empty());
}
