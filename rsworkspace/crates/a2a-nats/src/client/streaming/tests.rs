use a2a::types::{SendMessageResponse, Task, TaskState, TaskStatus};
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::mocks::{MockJetStreamConsumer, MockJetStreamConsumerFactory};

use super::*;

fn test_prefix() -> A2aPrefix {
    A2aPrefix::new("a2a".to_string()).unwrap()
}

fn test_req_id() -> ReqId {
    ReqId::from_test("req-stream-1")
}

fn bootstrap_success(task_id: &str) -> Bytes {
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
    let json = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "req-stream-1",
        "result": response
    });
    serde_json::to_vec(&json).unwrap().into()
}

fn bootstrap_error(code: i32, msg: &str) -> Bytes {
    let json = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "req-stream-1",
        "error": { "code": code, "message": msg }
    });
    serde_json::to_vec(&json).unwrap().into()
}

#[derive(serde::Serialize)]
struct TestParams {
    dummy: String,
}

fn make_ctx<'a>(
    nats: &'a AdvancedMockNatsClient,
    js: &'a MockJetStreamConsumerFactory,
    req_id: &'a ReqId,
    prefix: &'a A2aPrefix,
    timeout: std::time::Duration,
) -> StreamingRequest<'a, AdvancedMockNatsClient, MockJetStreamConsumerFactory> {
    StreamingRequest {
        nats,
        js,
        subject: "a2a.agents.bot.message.stream",
        method: "message/stream",
        req_id,
        prefix,
        op_timeout: timeout,
        gateway_caller_jwt: None,
    }
}

#[tokio::test]
async fn bootstrap_success_returns_task_and_stream() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.agents.bot.message.stream", bootstrap_success("task-abc"));

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);

    let req_id = test_req_id();
    let prefix = test_prefix();
    let (envelope, _stream) = send_streaming(
        make_ctx(&nats, &js, &req_id, &prefix, std::time::Duration::from_secs(5)),
        &TestParams { dummy: "hi".into() },
    )
    .await
    .unwrap();

    assert!(matches!(envelope, SendMessageResponse::Task(_)));
}

#[tokio::test]
async fn bootstrap_error_propagates_as_client_error() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.agents.bot.message.stream", bootstrap_error(-32001, "not found"));

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);

    let req_id = test_req_id();
    let prefix = test_prefix();
    let result = send_streaming(
        make_ctx(&nats, &js, &req_id, &prefix, std::time::Duration::from_secs(5)),
        &TestParams { dummy: "hi".into() },
    )
    .await;

    assert!(matches!(result, Err(ClientError::TaskNotFound)));
}

#[tokio::test]
async fn nats_transport_failure_returns_transport_error() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_request();

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);

    let req_id = test_req_id();
    let prefix = test_prefix();
    let result = send_streaming(
        make_ctx(&nats, &js, &req_id, &prefix, std::time::Duration::from_secs(5)),
        &TestParams { dummy: "hi".into() },
    )
    .await;

    assert!(matches!(result, Err(ClientError::Transport(_))));
}

#[tokio::test]
async fn get_stream_failure_returns_consumer_setup_error() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.agents.bot.message.stream", bootstrap_success("t1"));

    let js = MockJetStreamConsumerFactory::new();
    js.fail_get_stream_at(1);

    let req_id = test_req_id();
    let prefix = test_prefix();
    let result = send_streaming(
        make_ctx(&nats, &js, &req_id, &prefix, std::time::Duration::from_secs(5)),
        &TestParams { dummy: "hi".into() },
    )
    .await;

    assert!(matches!(result, Err(ClientError::ConsumerSetup(_))));
}

#[tokio::test]
async fn open_task_stream_returns_typed_event_stream() {
    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);

    let req_id = test_req_id();

    let stream = open_task_stream(&js, &test_prefix(), &req_id).await;
    assert!(stream.is_ok());
}

#[tokio::test]
async fn open_task_stream_get_stream_failure_returns_error() {
    let js = MockJetStreamConsumerFactory::new();
    js.fail_get_stream_at(1);

    let req_id = test_req_id();

    let result = open_task_stream(&js, &test_prefix(), &req_id).await;
    assert!(matches!(result, Err(ClientError::ConsumerSetup(_))));
}

#[tokio::test]
async fn hang_returns_timeout_error() {
    let nats = AdvancedMockNatsClient::new();
    nats.hang_next_request();

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);

    let req_id = test_req_id();
    let prefix = test_prefix();
    let result = send_streaming(
        make_ctx(&nats, &js, &req_id, &prefix, std::time::Duration::from_millis(10)),
        &TestParams { dummy: "hi".into() },
    )
    .await;

    assert!(matches!(result, Err(ClientError::Timeout { .. })));
}

#[tokio::test]
async fn gateway_jwt_attaches_caller_jwt_to_bootstrap() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.gateway.bot.message.stream", bootstrap_success("task-gw"));

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);

    let jwt = MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjk5OTk5OTk5OTl9.signature").unwrap();
    let req_id = test_req_id();
    let prefix = test_prefix();
    let ctx = StreamingRequest {
        nats: &nats,
        js: &js,
        subject: "a2a.gateway.bot.message.stream",
        method: "message/stream",
        req_id: &req_id,
        prefix: &prefix,
        op_timeout: std::time::Duration::from_secs(5),
        gateway_caller_jwt: Some(&jwt),
    };
    let (envelope, _stream) = send_streaming(ctx, &TestParams { dummy: "hi".into() }).await.unwrap();
    assert!(matches!(envelope, SendMessageResponse::Task(_)));
}
