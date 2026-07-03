use a2a::types::{CancelTaskRequest, Task, TaskState, TaskStatus};
use bytes::Bytes;
use jsonrpc_nats::{Message, ResponseId, encode};
use trogon_nats::AdvancedMockNatsClient;

use super::*;

fn cancel_task_request(id: &str) -> CancelTaskRequest {
    CancelTaskRequest {
        id: id.to_string(),
        metadata: None,
        tenant: None,
    }
}

fn task_response(task_id: &str, state: TaskState) -> (async_nats::HeaderMap, Bytes) {
    let task = Task {
        id: task_id.to_string(),
        context_id: String::new(),
        status: TaskStatus {
            state,
            message: None,
            timestamp: None,
        },
        artifacts: None,
        history: None,
        metadata: None,
    };
    let encoded = encode(&Message::Success {
        id: ResponseId::String("any".into()),
        result: serde_json::json!(task),
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
async fn tasks_cancel_targets_agent_subject_by_default() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = task_response("t-1", TaskState::Canceled);
    nats.set_response_wire("a2a.agents.test-agent.tasks.cancel", headers, body);
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    let task = client.tasks_cancel(&cancel_task_request("t-1")).await.unwrap();
    assert_eq!(task.status.state, TaskState::Canceled);
}

#[tokio::test]
async fn tasks_cancel_targets_gateway_subject_under_gateway_routing() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = task_response("t-gw", TaskState::Canceled);
    nats.set_response_wire("a2a.gateway.test-agent.tasks.cancel", headers, body);
    let jwt = MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjk5OTk5OTk5OTl9.signature").unwrap();
    let client = A2aClient::new(prefix(), agent_id(), nats, ()).routing_via_gateway_ingress(jwt);
    client.tasks_cancel(&cancel_task_request("t-gw")).await.unwrap();
}

#[tokio::test]
async fn tasks_cancel_propagates_task_not_cancelable() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = error_response(-32002, "terminal");
    nats.set_response_wire("a2a.agents.test-agent.tasks.cancel", headers, body);
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    assert!(matches!(
        client.tasks_cancel(&cancel_task_request("t")).await,
        Err(ClientError::TaskNotCancelable)
    ));
}

#[tokio::test]
async fn tasks_cancel_propagates_transport_errors() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_request();
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    assert!(matches!(
        client.tasks_cancel(&cancel_task_request("t")).await,
        Err(ClientError::Transport(_))
    ));
}
