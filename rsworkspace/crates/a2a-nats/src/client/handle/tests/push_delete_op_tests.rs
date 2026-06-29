use a2a::types::DeleteTaskPushNotificationConfigRequest;
use bytes::Bytes;
use jsonrpc_nats::{Message, ResponseId, encode};
use trogon_nats::AdvancedMockNatsClient;

use super::*;

fn delete_request(id: &str) -> DeleteTaskPushNotificationConfigRequest {
    DeleteTaskPushNotificationConfigRequest {
        task_id: "task-1".to_string(),
        id: id.to_string(),
        tenant: None,
    }
}

fn ok_response() -> (async_nats::HeaderMap, Bytes) {
    let encoded = encode(&Message::Success {
        id: ResponseId::String("any".into()),
        result: serde_json::json!(null),
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
async fn push_delete_targets_agent_subject_by_default() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = ok_response();
    nats.set_response_wire("a2a.agents.test-agent.push.delete", headers, body);
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    client.push_delete(&delete_request("c-1")).await.unwrap();
}

#[tokio::test]
async fn push_delete_targets_gateway_subject_under_gateway_routing() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = ok_response();
    nats.set_response_wire("a2a.gateway.test-agent.push.delete", headers, body);
    let jwt = MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjk5OTk5OTk5OTl9.signature").unwrap();
    let client = A2aClient::new(prefix(), agent_id(), nats, ()).routing_via_gateway_ingress(jwt);
    client.push_delete(&delete_request("c-gw")).await.unwrap();
}

#[tokio::test]
async fn push_delete_propagates_task_not_found() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = error_response(-32001, "missing");
    nats.set_response_wire("a2a.agents.test-agent.push.delete", headers, body);
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    assert!(matches!(
        client.push_delete(&delete_request("c")).await,
        Err(ClientError::TaskNotFound)
    ));
}

#[tokio::test]
async fn push_delete_propagates_transport_errors() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_request();
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    assert!(matches!(
        client.push_delete(&delete_request("c")).await,
        Err(ClientError::Transport(_))
    ));
}
