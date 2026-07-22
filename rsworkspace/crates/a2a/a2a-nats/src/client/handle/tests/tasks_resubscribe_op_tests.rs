use a2a::types::{Task, TaskState, TaskStatus};
use bytes::Bytes;
use jsonrpc_nats::{Message, ResponseId, encode};
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::mocks::{MockJetStreamConsumer, MockJetStreamConsumerFactory};

use super::*;

fn task_id() -> A2aTaskId {
    A2aTaskId::new("task-resub-1").unwrap()
}

fn task_snapshot(task_id: &str) -> (async_nats::HeaderMap, Bytes) {
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
async fn tasks_resubscribe_returns_snapshot_and_stream() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = task_snapshot("task-resub-1");
    nats.set_response_wire("a2a.agents.test-agent.tasks.resubscribe", headers, body);
    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);
    let client = A2aClient::new(prefix(), agent_id(), nats, js);
    let (snapshot, stream) = client.tasks_resubscribe(&task_id(), 42).await.unwrap();
    assert_eq!(snapshot.id, "task-resub-1");
    assert_eq!(stream.last_seq(), 42);
}

#[tokio::test]
async fn tasks_resubscribe_targets_gateway_subject_under_gateway_routing() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = task_snapshot("task-gw");
    nats.set_response_wire("a2a.gateway.test-agent.tasks.resubscribe", headers, body);
    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);
    let jwt = MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjk5OTk5OTk5OTl9.signature").unwrap();
    let client = A2aClient::new(prefix(), agent_id(), nats, js).routing_via_gateway_ingress(jwt);
    let (snapshot, _stream) = client.tasks_resubscribe(&task_id(), 0).await.unwrap();
    assert_eq!(snapshot.id, "task-gw");
}

#[tokio::test]
async fn tasks_resubscribe_propagates_task_not_found() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = error_response(-32001, "missing");
    nats.set_response_wire("a2a.agents.test-agent.tasks.resubscribe", headers, body);
    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);
    let client = A2aClient::new(prefix(), agent_id(), nats, js);
    assert!(matches!(
        client.tasks_resubscribe(&task_id(), 0).await,
        Err(ClientError::TaskNotFound)
    ));
}

#[tokio::test]
async fn tasks_resubscribe_propagates_transport_errors() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_request();
    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);
    let client = A2aClient::new(prefix(), agent_id(), nats, js);
    assert!(matches!(
        client.tasks_resubscribe(&task_id(), 0).await,
        Err(ClientError::Transport(_))
    ));
}
