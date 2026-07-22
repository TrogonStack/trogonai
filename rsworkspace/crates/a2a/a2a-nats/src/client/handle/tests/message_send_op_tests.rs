use a2a::types::{Message, Role, SendMessageRequest, Task, TaskState, TaskStatus};
use bytes::Bytes;
use jsonrpc_nats::{Message as JsonRpcMessage, ResponseId, encode};
use trogon_nats::AdvancedMockNatsClient;

use super::*;

fn send_message_request() -> SendMessageRequest {
    SendMessageRequest {
        message: Message {
            message_id: "m-1".to_string(),
            role: Role::User,
            parts: vec![],
            context_id: None,
            task_id: None,
            reference_task_ids: None,
            extensions: None,
            metadata: None,
        },
        configuration: None,
        metadata: None,
        tenant: None,
    }
}

fn task_response(task_id: &str) -> (async_nats::HeaderMap, Bytes) {
    let task = Task {
        id: task_id.to_string(),
        context_id: String::new(),
        status: TaskStatus {
            state: TaskState::Working,
            message: None,
            timestamp: None,
        },
        artifacts: None,
        history: None,
        metadata: None,
    };
    let response = a2a::types::SendMessageResponse::Task(task);
    let encoded = encode(&JsonRpcMessage::Success {
        id: ResponseId::String("any".into()),
        result: serde_json::json!(response),
    })
    .unwrap();
    (encoded.headers, encoded.body)
}

fn error_response(code: i32, msg: &str) -> (async_nats::HeaderMap, Bytes) {
    let encoded = encode(&JsonRpcMessage::Error {
        id: ResponseId::String("any".into()),
        code,
        message: msg.to_string(),
        data: None,
    })
    .unwrap();
    (encoded.headers, encoded.body)
}

#[tokio::test]
async fn message_send_targets_agent_subject_by_default() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = task_response("t-1");
    nats.set_response_wire("a2a.agents.test-agent.message.send", headers, body);
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    let resp = client.message_send(&send_message_request()).await.unwrap();
    assert!(matches!(resp, a2a::types::SendMessageResponse::Task(_)));
}

#[tokio::test]
async fn message_send_targets_gateway_subject_under_gateway_routing() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = task_response("t-gw");
    nats.set_response_wire("a2a.gateway.test-agent.message.send", headers, body);
    let jwt = MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjk5OTk5OTk5OTl9.signature").unwrap();
    let client = A2aClient::new(prefix(), agent_id(), nats, ()).routing_via_gateway_ingress(jwt);
    let resp = client.message_send(&send_message_request()).await.unwrap();
    assert!(matches!(resp, a2a::types::SendMessageResponse::Task(_)));
}

#[tokio::test]
async fn message_send_propagates_typed_jsonrpc_errors() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = error_response(-32050, "down");
    nats.set_response_wire("a2a.agents.test-agent.message.send", headers, body);
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    let err = client.message_send(&send_message_request()).await.unwrap_err();
    assert!(matches!(err, ClientError::AgentUnavailable));
}

#[tokio::test]
async fn message_send_propagates_transport_errors() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_request();
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    assert!(matches!(
        client.message_send(&send_message_request()).await,
        Err(ClientError::Transport(_))
    ));
}
