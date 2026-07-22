use super::*;
use agent_client_protocol::schema::v1::PromptResponse;
use bytes::Bytes;
use jsonrpc_nats::{Message, ResponseId, encode};
use trogon_nats::jetstream::mocks::*;

use crate::agent::test_support::MockJs;
use crate::req_id::ReqId;
use crate::session_id::AcpSessionId;

fn test_prefix() -> AcpPrefix {
    AcpPrefix::new("acp").expect("test prefix")
}

fn test_sid(s: &str) -> AcpSessionId {
    AcpSessionId::new(s).expect("test session id")
}

fn prompt_request() -> agent_client_protocol::schema::v1::PromptRequest {
    agent_client_protocol::schema::v1::PromptRequest::new("s1", vec![])
}

fn make_nats_msg(payload: &[u8], headers: Option<async_nats::HeaderMap>) -> async_nats::Message {
    async_nats::Message {
        subject: "test".into(),
        reply: None,
        payload: Bytes::from(payload.to_vec()),
        headers,
        status: None,
        description: None,
        length: payload.len(),
    }
}

fn make_wire_success_msg<Res: serde::Serialize>(req_id: &str, result: &Res) -> async_nats::Message {
    let encoded = encode(&Message::Success {
        id: ResponseId::String(req_id.to_string()),
        result: serde_json::to_value(result).unwrap(),
    })
    .unwrap();
    make_nats_msg(&encoded.body, Some(encoded.headers))
}

fn make_wire_error_msg(req_id: &str, error: &agent_client_protocol::Error) -> async_nats::Message {
    let encoded = encode(&Message::Error {
        id: ResponseId::String(req_id.to_string()),
        code: i32::from(error.code),
        message: error.message.clone(),
        data: error.data.clone(),
    })
    .unwrap();
    make_nats_msg(&encoded.body, Some(encoded.headers))
}

#[tokio::test]
async fn js_request_success() {
    let js = MockJs::new();
    let (consumer, tx) = MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(consumer);

    let response = PromptResponse::new(agent_client_protocol::schema::v1::StopReason::EndTurn);
    let msg = MockJsMessage::new(make_wire_success_msg("req-1", &response));
    tx.unbounded_send(Ok(msg)).unwrap();

    let result: agent_client_protocol::Result<PromptResponse> = js_request(
        &js,
        "acp.session.s1.agent.prompt",
        "prompt",
        &prompt_request(),
        &test_prefix(),
        &test_sid("s1"),
        &ReqId::from_test("req-1"),
        Duration::from_secs(5),
    )
    .await;

    assert!(result.is_ok());
    assert_eq!(
        result.unwrap().stop_reason,
        agent_client_protocol::schema::v1::StopReason::EndTurn
    );
}

#[tokio::test]
async fn js_request_publish_failure() {
    let js = MockJs::new();
    let (consumer, _tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(consumer);
    js.publisher.fail_next_js_publish();

    let result: agent_client_protocol::Result<PromptResponse> = js_request(
        &js,
        "acp.session.s1.agent.prompt",
        "prompt",
        &prompt_request(),
        &test_prefix(),
        &test_sid("s1"),
        &ReqId::from_test("req-1"),
        Duration::from_secs(5),
    )
    .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("js publish"));
}

#[tokio::test]
async fn js_request_consumer_creation_failure() {
    let js = MockJs::new();

    let result: agent_client_protocol::Result<PromptResponse> = js_request(
        &js,
        "acp.session.s1.agent.prompt",
        "prompt",
        &prompt_request(),
        &test_prefix(),
        &test_sid("s1"),
        &ReqId::from_test("req-1"),
        Duration::from_secs(5),
    )
    .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("create response consumer"));
}

#[tokio::test]
async fn js_request_messages_failure() {
    let js = MockJs::new();
    let failing_consumer = MockJetStreamConsumer::failing();
    js.consumer_factory.add_consumer(failing_consumer);

    let result: agent_client_protocol::Result<PromptResponse> = js_request(
        &js,
        "acp.session.s1.agent.prompt",
        "prompt",
        &prompt_request(),
        &test_prefix(),
        &test_sid("s1"),
        &ReqId::from_test("req-1"),
        Duration::from_secs(5),
    )
    .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("response messages"));
}

#[tokio::test]
async fn js_request_bad_response_payload() {
    let js = MockJs::new();
    let (consumer, tx) = MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(consumer);

    let msg = MockJsMessage::new(make_nats_msg(b"not json", None));
    tx.unbounded_send(Ok(msg)).unwrap();

    let result: agent_client_protocol::Result<PromptResponse> = js_request(
        &js,
        "acp.session.s1.agent.prompt",
        "prompt",
        &prompt_request(),
        &test_prefix(),
        &test_sid("s1"),
        &ReqId::from_test("req-1"),
        Duration::from_secs(5),
    )
    .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("bad response payload"));
}

#[tokio::test]
async fn js_request_timeout() {
    let js = MockJs::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(consumer);

    let result: agent_client_protocol::Result<PromptResponse> = js_request(
        &js,
        "acp.session.s1.agent.prompt",
        "prompt",
        &prompt_request(),
        &test_prefix(),
        &test_sid("s1"),
        &ReqId::from_test("req-1"),
        Duration::from_millis(10),
    )
    .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("timed out"));
}

#[tokio::test]
async fn js_request_agent_error_response() {
    let js = MockJs::new();
    let (consumer, tx) = MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(consumer);

    let agent_err =
        agent_client_protocol::Error::new(agent_client_protocol::ErrorCode::InternalError.into(), "agent failed");
    let msg = MockJsMessage::new(make_wire_error_msg("req-1", &agent_err));
    tx.unbounded_send(Ok(msg)).unwrap();

    let result: agent_client_protocol::Result<PromptResponse> = js_request(
        &js,
        "acp.session.s1.agent.prompt",
        "prompt",
        &prompt_request(),
        &test_prefix(),
        &test_sid("s1"),
        &ReqId::from_test("req-1"),
        Duration::from_secs(5),
    )
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.message.contains("agent failed"));
}

#[tokio::test]
async fn js_request_stream_closed() {
    let js = MockJs::new();
    let (consumer, tx) = MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(consumer);

    drop(tx);

    let result: agent_client_protocol::Result<PromptResponse> = js_request(
        &js,
        "acp.session.s1.agent.prompt",
        "prompt",
        &prompt_request(),
        &test_prefix(),
        &test_sid("s1"),
        &ReqId::from_test("req-1"),
        Duration::from_secs(5),
    )
    .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("response stream closed"));
}

#[tokio::test]
async fn js_request_consumer_stream_error() {
    let js = MockJs::new();
    let (consumer, tx) = MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(consumer);

    tx.unbounded_send(Err(trogon_nats::mocks::MockError("stream error".to_string())))
        .unwrap();

    let result: agent_client_protocol::Result<PromptResponse> = js_request(
        &js,
        "acp.session.s1.agent.prompt",
        "prompt",
        &prompt_request(),
        &test_prefix(),
        &test_sid("s1"),
        &ReqId::from_test("req-1"),
        Duration::from_secs(5),
    )
    .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("response consumer"));
}
