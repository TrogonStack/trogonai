use trogon_nats::AdvancedMockNatsClient;

use super::*;
use crate::server::test_support::{parse_response, stub};

fn get_request(id: &str) -> a2a::types::GetTaskPushNotificationConfigRequest {
    a2a::types::GetTaskPushNotificationConfigRequest {
        task_id: "task-1".to_string(),
        id: id.to_string(),
        tenant: None,
    }
}

fn config(id: &str) -> a2a::types::TaskPushNotificationConfig {
    a2a::types::TaskPushNotificationConfig {
        url: "https://example.com/webhook".to_string(),
        id: Some(id.to_string()),
        task_id: "task-1".to_string(),
        token: None,
        authentication: None,
        tenant: None,
    }
}

fn get_payload(req_id: i64, cfg_id: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": req_id,
        "method": "tasks/pushNotificationConfig/get",
        "params": get_request(cfg_id)
    }))
    .unwrap()
}

#[tokio::test]
async fn success_publishes_config() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    handler.lock().unwrap().push_get_result = Some(Ok(config("c-1")));
    handle(&handler, &get_payload(1, "c-1"), Some("r".into()), &nats).await;
    let body = parse_response(&nats.published_payloads()[0]);
    assert_eq!(body["result"]["id"].as_str(), Some("c-1"));
}

#[tokio::test]
async fn task_not_found_error_uses_typed_code() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    handler.lock().unwrap().push_get_result = Some(Err(A2aError::task_not_found("missing")));
    handle(&handler, &get_payload(2, "c"), Some("r".into()), &nats).await;
    let body = parse_response(&nats.published_payloads()[0]);
    assert_eq!(
        body["error"]["code"].as_i64(),
        Some(i64::from(crate::error::TASK_NOT_FOUND))
    );
}

#[tokio::test]
async fn no_reply_drops_request() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    handle(&handler, &get_payload(3, "c"), None, &nats).await;
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn missing_params_returns_invalid_params_error() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    let payload = serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tasks/pushNotificationConfig/get"
    }))
    .unwrap();
    handle(&handler, &payload, Some("r".into()), &nats).await;
    let body = parse_response(&nats.published_payloads()[0]);
    assert_eq!(body["error"]["code"], -32602);
}

#[tokio::test]
async fn invalid_params_shape_returns_invalid_params_code() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    let payload = serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tasks/pushNotificationConfig/get",
        "params": { "taskId": 42 }
    }))
    .unwrap();
    handle(&handler, &payload, Some("r".into()), &nats).await;
    let body = parse_response(&nats.published_payloads()[0]);
    assert_eq!(body["error"]["code"], -32602);
    assert_eq!(body["id"], 6);
}

#[tokio::test]
async fn malformed_json_still_publishes_parse_error_with_null_id() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    handle(&handler, b"not json", Some("r".into()), &nats).await;
    let body = parse_response(&nats.published_payloads()[0]);
    assert_eq!(body["error"]["code"], -32700);
    assert!(body["id"].is_null());
}

#[tokio::test]
async fn notification_without_id_is_dropped() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    let payload = serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tasks/pushNotificationConfig/get",
        "params": get_request("c")
    }))
    .unwrap();
    handle(&handler, &payload, Some("r".into()), &nats).await;
    assert!(nats.published_messages().is_empty());
}
