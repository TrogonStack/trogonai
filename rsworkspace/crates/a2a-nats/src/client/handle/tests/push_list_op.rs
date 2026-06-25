use a2a::types::{ListTaskPushNotificationConfigsRequest, ListTaskPushNotificationConfigsResponse};
use bytes::Bytes;
use jsonrpc_nats::{Message, ResponseId, encode};
use trogon_nats::AdvancedMockNatsClient;

use super::*;

fn list_request() -> ListTaskPushNotificationConfigsRequest {
    ListTaskPushNotificationConfigsRequest {
        task_id: "task-1".to_string(),
        page_size: None,
        page_token: None,
        tenant: None,
    }
}

fn list_response() -> (async_nats::HeaderMap, Bytes) {
    let response = ListTaskPushNotificationConfigsResponse {
        configs: vec![],
        next_page_token: None,
    };
    let encoded = encode(&Message::Success {
        id: ResponseId::String("any".into()),
        result: serde_json::json!(response),
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
async fn push_list_targets_agent_subject_by_default() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = list_response();
    nats.set_response_wire("a2a.agents.test-agent.push.list", headers, body);
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    let resp = client.push_list(&list_request()).await.unwrap();
    assert!(resp.configs.is_empty());
}

#[tokio::test]
async fn push_list_targets_gateway_subject_under_gateway_routing() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = list_response();
    nats.set_response_wire("a2a.gateway.test-agent.push.list", headers, body);
    let jwt = MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjk5OTk5OTk5OTl9.signature").unwrap();
    let client = A2aClient::new(prefix(), agent_id(), nats, ()).routing_via_gateway_ingress(jwt);
    client.push_list(&list_request()).await.unwrap();
}

#[tokio::test]
async fn push_list_propagates_typed_jsonrpc_errors() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = error_response(-32003, "not supported");
    nats.set_response_wire("a2a.agents.test-agent.push.list", headers, body);
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    assert!(matches!(
        client.push_list(&list_request()).await,
        Err(ClientError::PushNotificationNotSupported)
    ));
}

#[tokio::test]
async fn push_list_propagates_transport_errors() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_request();
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    assert!(matches!(
        client.push_list(&list_request()).await,
        Err(ClientError::Transport(_))
    ));
}
