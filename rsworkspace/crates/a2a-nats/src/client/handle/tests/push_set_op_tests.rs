use a2a::types::TaskPushNotificationConfig;
use bytes::Bytes;
use jsonrpc_nats::{Message, ResponseId, encode};
use trogon_nats::AdvancedMockNatsClient;

use super::*;

fn push_config(id: &str) -> TaskPushNotificationConfig {
    TaskPushNotificationConfig {
        url: "https://example.com/webhook".to_string(),
        id: Some(id.to_string()),
        task_id: "task-1".to_string(),
        token: None,
        authentication: None,
        tenant: None,
    }
}

fn push_response(id: &str) -> (async_nats::HeaderMap, Bytes) {
    let encoded = encode(&Message::Success {
        id: ResponseId::String("any".into()),
        result: serde_json::json!(push_config(id)),
    })
    .unwrap();
    (encoded.headers, encoded.body)
}

fn error_response(code: i32, msg: &str) -> (async_nats::HeaderMap, Bytes) {
    let encoded = encode(&Message::Error {
        id: ResponseId::String("any".into()),
        code,
        message: msg.to_string(),
        data: None,
    })
    .unwrap();
    (encoded.headers, encoded.body)
}

#[tokio::test]
async fn push_set_targets_agent_subject_by_default() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = push_response("c-1");
    nats.set_response_wire("a2a.agents.test-agent.push.set", headers, body);
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    let cfg = client.push_set(&push_config("c-1")).await.unwrap();
    assert_eq!(cfg.id.as_deref(), Some("c-1"));
}

#[tokio::test]
async fn push_set_targets_gateway_subject_under_gateway_routing() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = push_response("c-gw");
    nats.set_response_wire("a2a.gateway.test-agent.push.set", headers, body);
    let jwt = MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjk5OTk5OTk5OTl9.signature").unwrap();
    let client = A2aClient::new(prefix(), agent_id(), nats, ()).routing_via_gateway_ingress(jwt);
    client.push_set(&push_config("c-gw")).await.unwrap();
}

#[tokio::test]
async fn push_set_propagates_push_not_supported_error() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = error_response(-32003, "not supported");
    nats.set_response_wire("a2a.agents.test-agent.push.set", headers, body);
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    assert!(matches!(
        client.push_set(&push_config("c")).await,
        Err(ClientError::PushNotificationNotSupported)
    ));
}

#[tokio::test]
async fn push_set_propagates_transport_errors() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_request();
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    assert!(matches!(
        client.push_set(&push_config("c")).await,
        Err(ClientError::Transport(_))
    ));
}
