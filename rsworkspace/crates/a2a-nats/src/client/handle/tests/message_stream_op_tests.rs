use a2a::types::{Message, Role, SendMessageRequest, SendMessageResponse, Task, TaskState, TaskStatus};
use bytes::Bytes;
use jsonrpc_nats::{Message as JsonRpcMessage, ResponseId, encode};
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::mocks::{MockJetStreamConsumer, MockJetStreamConsumerFactory};

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

fn bootstrap_response(task_id: &str) -> (async_nats::HeaderMap, Bytes) {
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
    let response = SendMessageResponse::Task(task);
    let encoded = encode(&JsonRpcMessage::Success {
        id: ResponseId::String("any".into()),
        result: serde_json::json!(response),
    })
    .unwrap();
    (encoded.headers, encoded.body)
}

#[tokio::test]
async fn message_stream_targets_agent_subject_by_default() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = bootstrap_response("t-1");
    nats.set_response_wire("a2a.agents.test-agent.message.stream", headers, body);
    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);
    let client = A2aClient::new(prefix(), agent_id(), nats, js);
    let (envelope, _stream) = client.message_stream(&send_message_request()).await.unwrap();
    assert!(matches!(envelope, SendMessageResponse::Task(_)));
}

#[tokio::test]
async fn message_stream_targets_gateway_subject_under_gateway_routing() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = bootstrap_response("t-gw");
    nats.set_response_wire("a2a.gateway.test-agent.message.stream", headers, body);
    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);
    let jwt = MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjk5OTk5OTk5OTl9.signature").unwrap();
    let client = A2aClient::new(prefix(), agent_id(), nats, js).routing_via_gateway_ingress(jwt);
    let (envelope, _stream) = client.message_stream(&send_message_request()).await.unwrap();
    assert!(matches!(envelope, SendMessageResponse::Task(_)));
}

#[tokio::test]
async fn message_stream_propagates_transport_errors() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_request();
    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);
    let client = A2aClient::new(prefix(), agent_id(), nats, js);
    assert!(matches!(
        client.message_stream(&send_message_request()).await,
        Err(ClientError::Transport(_))
    ));
}
