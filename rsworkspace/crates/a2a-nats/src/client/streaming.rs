use std::sync::{Arc, Mutex};

use a2a_types::SendMessageResponse;
use bytes::Bytes;
use serde::Serialize;
use tokio::time::timeout;
use trogon_nats::RequestClient;
use trogon_nats::jetstream::{JetStreamCreateConsumer, JetStreamGetStream, JsAck, JsMessageOf, JsMessageRef};

use crate::a2a_prefix::A2aPrefix;
use crate::constants::REQ_ID_HEADER;
use crate::jetstream::consumers::stream_events_consumer;
use crate::jetstream::streams::events_stream_name;
use crate::jsonrpc::JsonRpcId;
use crate::req_id::ReqId;

use super::error::ClientError;
use super::event_stream::{TypedEventStream, build_event_stream};
use super::wire::{JsonRpcRequest, JsonRpcResponse};

pub struct StreamingRequest<'a, N, J> {
    pub nats: &'a N,
    pub js: &'a J,
    pub subject: &'a str,
    pub method: &'static str,
    pub req_id: &'a ReqId,
    pub prefix: &'a A2aPrefix,
    pub op_timeout: std::time::Duration,
}

pub async fn send_streaming<N, J, Req>(
    ctx: StreamingRequest<'_, N, J>,
    params: &Req,
) -> Result<(SendMessageResponse, TypedEventStream), ClientError>
where
    N: RequestClient,
    J: JetStreamGetStream,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
    Req: Serialize,
{
    let StreamingRequest { nats, js, subject, method, req_id, prefix, op_timeout } = ctx;
    let envelope = JsonRpcRequest::new(JsonRpcId::String(req_id.as_str().to_owned()), method, params);
    let payload = serde_json::to_vec(&envelope).map_err(ClientError::Serialize)?;

    let stream_name = events_stream_name(prefix);
    let stream = js
        .get_stream(&stream_name)
        .await
        .map_err(|e| ClientError::ConsumerSetup(format!("get stream '{stream_name}': {e}")))?;

    let consumer_config = stream_events_consumer(prefix, req_id);

    let last_seq = Arc::new(Mutex::new(0u64));

    let consumer = stream
        .create_consumer(consumer_config)
        .await
        .map_err(|e| ClientError::ConsumerSetup(format!("create consumer: {e}")))?;

    let event_stream = build_event_stream(consumer, last_seq);

    let mut headers = async_nats::HeaderMap::new();
    headers.insert(REQ_ID_HEADER, req_id.as_str());

    let msg = timeout(
        op_timeout,
        nats.request_with_headers(subject.to_string(), headers, Bytes::from(payload)),
    )
    .await
    .map_err(|_| ClientError::Timeout { subject: subject.to_string() })?
    .map_err(|e| ClientError::Transport(e.to_string()))?;

    let response: JsonRpcResponse<SendMessageResponse> =
        serde_json::from_slice(&msg.payload).map_err(ClientError::Deserialize)?;

    match response {
        JsonRpcResponse::Success(s) => Ok((s.result, event_stream)),
        JsonRpcResponse::Error(e) => Err(ClientError::from_jsonrpc_code(e.error.code, e.error.message)),
    }
}

pub async fn open_task_stream<J>(
    js: &J,
    prefix: &A2aPrefix,
    req_id: &ReqId,
) -> Result<TypedEventStream, ClientError>
where
    J: JetStreamGetStream,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    let stream_name = events_stream_name(prefix);
    let stream = js
        .get_stream(&stream_name)
        .await
        .map_err(|e| ClientError::ConsumerSetup(format!("get stream '{stream_name}': {e}")))?;

    let consumer_config = stream_events_consumer(prefix, req_id);

    let last_seq = Arc::new(Mutex::new(0u64));

    let consumer = stream
        .create_consumer(consumer_config)
        .await
        .map_err(|e| ClientError::ConsumerSetup(format!("create consumer: {e}")))?;

    Ok(build_event_stream(consumer, last_seq))
}

#[cfg(test)]
mod tests {
    use super::*;
    use a2a_types::{SendMessageResponse, Task, TaskState, TaskStatus};
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::mocks::{MockJetStreamConsumer, MockJetStreamConsumerFactory};

    fn test_prefix() -> A2aPrefix {
        A2aPrefix::new("a2a".to_string()).unwrap()
    }

    fn test_req_id() -> ReqId {
        ReqId::from_test("req-stream-1")
    }

    fn bootstrap_success(task_id: &str) -> bytes::Bytes {
        let task = Task {
            id: task_id.to_string(),
            status: Some(TaskStatus {
                state: TaskState::Working.into(),
                message: None,
                timestamp: None,
            }),
            ..Default::default()
        };
        let response = SendMessageResponse {
            payload: Some(a2a_types::send_message_response::Payload::Task(task)),
        };
        let json = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "req-stream-1",
            "result": response
        });
        serde_json::to_vec(&json).unwrap().into()
    }

    fn bootstrap_error(code: i32, msg: &str) -> bytes::Bytes {
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
            subject: "a2a.agent.bot.message.stream",
            method: "message/stream",
            req_id,
            prefix,
            op_timeout: timeout,
        }
    }

    #[tokio::test]
    async fn bootstrap_success_returns_task_and_stream() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agent.bot.message.stream", bootstrap_success("task-abc"));

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

        assert!(envelope.payload.is_some());
    }

    #[tokio::test]
    async fn bootstrap_error_propagates_as_client_error() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agent.bot.message.stream", bootstrap_error(-32001, "not found"));

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
        nats.set_response("a2a.agent.bot.message.stream", bootstrap_success("t1"));

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
}
