use a2a::types::{GetTaskRequest, Task, TaskState, TaskStatus};
use bytes::Bytes;
use trogon_nats::AdvancedMockNatsClient;

use super::*;

fn get_task_request(id: &str) -> GetTaskRequest {
    GetTaskRequest {
        id: id.to_string(),
        tenant: None,
        history_length: None,
    }
}

fn task_response(task_id: &str) -> Bytes {
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
    let json = serde_json::json!({"jsonrpc":"2.0","id":"any","result":task});
    serde_json::to_vec(&json).unwrap().into()
}

fn error_response(code: i32, msg: &str) -> Bytes {
    let json = serde_json::json!({"jsonrpc":"2.0","id":"any","error":{"code":code,"message":msg}});
    serde_json::to_vec(&json).unwrap().into()
}

#[tokio::test]
async fn tasks_get_targets_agent_subject_by_default() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.agents.test-agent.tasks.get", task_response("t-1"));
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    let task = client.tasks_get(&get_task_request("t-1")).await.unwrap();
    assert_eq!(task.id, "t-1");
}

#[tokio::test]
async fn tasks_get_targets_gateway_subject_under_gateway_routing() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.gateway.test-agent.tasks.get", task_response("t-gw"));
    let jwt = MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjk5OTk5OTk5OTl9.signature").unwrap();
    let client = A2aClient::new(prefix(), agent_id(), nats, ()).routing_via_gateway_ingress(jwt);
    let task = client.tasks_get(&get_task_request("t-gw")).await.unwrap();
    assert_eq!(task.id, "t-gw");
}

#[tokio::test]
async fn tasks_get_propagates_task_not_found() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.agents.test-agent.tasks.get", error_response(-32001, "missing"));
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    assert!(matches!(
        client.tasks_get(&get_task_request("nope")).await,
        Err(ClientError::TaskNotFound)
    ));
}

#[tokio::test]
async fn tasks_get_propagates_transport_errors() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_request();
    let client = A2aClient::new(prefix(), agent_id(), nats, ());
    assert!(matches!(
        client.tasks_get(&get_task_request("x")).await,
        Err(ClientError::Transport(_))
    ));
}
