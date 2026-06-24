use a2a::types::TaskPushNotificationConfig;
use bytes::Bytes;
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

fn push_response(id: &str) -> Bytes {
    let json = serde_json::json!({"jsonrpc":"2.0","id":"any","result":push_config(id)});
    serde_json::to_vec(&json).unwrap().into()
}

fn error_response(code: i32, msg: &str) -> Bytes {
    let json = serde_json::json!({"jsonrpc":"2.0","id":"any","error":{"code":code,"message":msg}});
    serde_json::to_vec(&json).unwrap().into()
}

#[tokio::test]
async fn push_set_targets_agent_subject_by_default() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.agents.test-agent.push.set", push_response("c-1"));
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    let cfg = client.push_set(&push_config("c-1")).await.unwrap();
    assert_eq!(cfg.id.as_deref(), Some("c-1"));
}

#[tokio::test]
async fn push_set_targets_gateway_subject_under_gateway_routing() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.gateway.test-agent.push.set", push_response("c-gw"));
    let jwt = MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjk5OTk5OTk5OTl9.signature").unwrap();
    let client = A2aClient::new(prefix(), agent_id(), nats, ()).routing_via_gateway_ingress(jwt);
    client.push_set(&push_config("c-gw")).await.unwrap();
}

#[tokio::test]
async fn push_set_propagates_push_not_supported_error() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.push.set",
        error_response(-32003, "not supported"),
    );
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
