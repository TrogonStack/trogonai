use jsonrpc_nats::RequestId;
use trogon_nats::AdvancedMockNatsClient;

use super::*;
use crate::server::test_support::{parse_published_response, stub, wire_notification, wire_request};

fn delete_request(id: &str) -> a2a::types::DeleteTaskPushNotificationConfigRequest {
    a2a::types::DeleteTaskPushNotificationConfigRequest {
        task_id: "task-1".to_string(),
        id: id.to_string(),
        tenant: None,
    }
}

fn delete_payload(req_id: i64, cfg_id: &str) -> (async_nats::HeaderMap, Vec<u8>) {
    wire_request(
        "tasks/pushNotificationConfig/delete",
        RequestId::Number(req_id),
        serde_json::to_value(delete_request(cfg_id)).unwrap(),
    )
}

#[tokio::test]
async fn success_publishes_null_result() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    handler.lock().unwrap().push_delete_result = Some(Ok(()));
    let (headers, payload) = delete_payload(1, "c-1");
    handle(&handler, &headers, &payload, Some("r".into()), &nats).await;
    let body = parse_published_response(&nats, 0);
    assert!(body["result"].is_null());
    assert!(body["error"].is_null());
}

#[tokio::test]
async fn task_not_found_error_uses_typed_code() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    handler.lock().unwrap().push_delete_result = Some(Err(A2aError::task_not_found("missing")));
    let (headers, payload) = delete_payload(2, "c");
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
    let (headers, payload) = delete_payload(3, "c");
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
        "tasks/pushNotificationConfig/delete",
        RequestId::Number(6),
        serde_json::json!({ "taskId": 42, "id": "c" }),
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
    let (headers, payload) = wire_notification(
        "tasks/pushNotificationConfig/delete",
        serde_json::json!({ "taskId": "task-1", "id": "c-1" }),
    );
    handle(&handler, &headers, &payload, Some("r".into()), &nats).await;
    assert!(nats.published_messages().is_empty());
}
