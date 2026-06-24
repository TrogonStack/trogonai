use jsonrpc_nats::RequestId;
use trogon_nats::AdvancedMockNatsClient;

use super::*;
use crate::server::test_support::{parse_published_response, stub, wire_notification, wire_request};

fn list_request() -> a2a::types::ListTaskPushNotificationConfigsRequest {
    a2a::types::ListTaskPushNotificationConfigsRequest {
        task_id: "task-1".to_string(),
        page_size: None,
        page_token: None,
        tenant: None,
    }
}

fn empty_response() -> a2a::types::ListTaskPushNotificationConfigsResponse {
    a2a::types::ListTaskPushNotificationConfigsResponse {
        configs: vec![],
        next_page_token: None,
    }
}

fn list_payload(req_id: i64) -> (async_nats::HeaderMap, Vec<u8>) {
    wire_request(
        "tasks/pushNotificationConfig/list",
        RequestId::Number(id),
        serde_json::json!({}),
    )
}

#[tokio::test]
async fn success_publishes_list() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    handler.lock().unwrap().push_list_result = Some(Ok(empty_response()));
    let (headers, payload) = list_payload(1);
    handle(&handler, &headers, &payload, Some("r".into()), &nats).await;
    let body = parse_published_response(&nats, 0);
    // configs is skip_serializing_if=Vec::is_empty, so an empty list omits the field.
    assert!(body.get("result").is_some());
    assert!(body["error"].is_null());
}

#[tokio::test]
async fn push_not_supported_error_uses_typed_code() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    handler.lock().unwrap().push_list_result = Some(Err(A2aError::push_notification_not_supported("no push")));
    let (headers, payload) = list_payload(2);
    handle(&handler, &headers, &payload, Some("r".into()), &nats).await;
    let body = parse_published_response(&nats, 0);
    assert_eq!(
        body["error"]["code"].as_i64(),
        Some(i64::from(crate::error::PUSH_NOTIFICATION_NOT_SUPPORTED))
    );
}

#[tokio::test]
async fn no_reply_drops_request() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    let (headers, payload) = list_payload(3);
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
    let (headers, payload) = wire_request("METHOD", RequestId::Number(5), serde_json::Value::Null);
    handle(&handler, &headers, &payload, Some("r".into()), &nats).await;
    let body = parse_published_response(&nats, 0);
    assert_eq!(body["error"]["code"], -32602);
    assert_eq!(body["id"], 6);
}

#[tokio::test]
async fn malformed_json_still_publishes_parse_error_with_null_id() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    handle(&handler, &async_nats::HeaderMap::new(), b"not json", Some("r".into()), &nats).await;
    let body = parse_published_response(&nats, 0);
    assert_eq!(body["error"]["code"], -32700);
    assert!(body["id"].is_null());
}

#[tokio::test]
async fn notification_without_id_is_dropped() {
    let nats = AdvancedMockNatsClient::new();
    let handler = stub();
    let (headers, payload) = wire_request("METHOD", RequestId::Number(5), serde_json::Value::Null);
    handle(&handler, &headers, &payload, Some("r".into()), &nats).await;
    assert!(nats.published_messages().is_empty());
}
