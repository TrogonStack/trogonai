use super::*;
use agent_client_protocol::PromptResponse;
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

fn make_nats_msg(payload: &[u8]) -> async_nats::Message {
    async_nats::Message {
        subject: "test".into(),
        reply: None,
        payload: Bytes::from(payload.to_vec()),
        headers: None,
        status: None,
        description: None,
        length: payload.len(),
    }
}

#[tokio::test]
async fn js_request_success() {
    let js = MockJs::new();
    let (consumer, tx) = MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(consumer);

    let response = PromptResponse::new(agent_client_protocol::StopReason::EndTurn);
    let msg = MockJsMessage::new(make_nats_msg(&serde_json::to_vec(&response).unwrap()));
    tx.unbounded_send(Ok(msg)).unwrap();

    let result: agent_client_protocol::Result<PromptResponse> = js_request(
        &js,
        "acp.session.s1.agent.prompt",
        &agent_client_protocol::PromptRequest::new("s1", vec![]),
        &trogon_std::StdJsonSerialize,
        &test_prefix(),
        &test_sid("s1"),
        &ReqId::from_test("req-1"),
        Duration::from_secs(5),
    )
    .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().stop_reason, agent_client_protocol::StopReason::EndTurn);
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
        &agent_client_protocol::PromptRequest::new("s1", vec![]),
        &trogon_std::StdJsonSerialize,
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
        &agent_client_protocol::PromptRequest::new("s1", vec![]),
        &trogon_std::StdJsonSerialize,
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
        &agent_client_protocol::PromptRequest::new("s1", vec![]),
        &trogon_std::StdJsonSerialize,
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

    let msg = MockJsMessage::new(make_nats_msg(b"not json"));
    tx.unbounded_send(Ok(msg)).unwrap();

    let result: agent_client_protocol::Result<PromptResponse> = js_request(
        &js,
        "acp.session.s1.agent.prompt",
        &agent_client_protocol::PromptRequest::new("s1", vec![]),
        &trogon_std::StdJsonSerialize,
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
        &agent_client_protocol::PromptRequest::new("s1", vec![]),
        &trogon_std::StdJsonSerialize,
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
    let msg = MockJsMessage::new(async_nats::Message {
        subject: "test".into(),
        reply: None,
        payload: Bytes::from(serde_json::to_vec(&agent_err).unwrap()),
        headers: None,
        status: None,
        description: None,
        length: 0,
    });
    tx.unbounded_send(Ok(msg)).unwrap();

    let result: agent_client_protocol::Result<PromptResponse> = js_request(
        &js,
        "acp.session.s1.agent.prompt",
        &agent_client_protocol::PromptRequest::new("s1", vec![]),
        &trogon_std::StdJsonSerialize,
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
        &agent_client_protocol::PromptRequest::new("s1", vec![]),
        &trogon_std::StdJsonSerialize,
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
        &agent_client_protocol::PromptRequest::new("s1", vec![]),
        &trogon_std::StdJsonSerialize,
        &test_prefix(),
        &test_sid("s1"),
        &ReqId::from_test("req-1"),
        Duration::from_secs(5),
    )
    .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("response consumer"));
}

#[tokio::test]
async fn js_request_serialize_failure() {
    let js = MockJs::new();
    let (consumer, _tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(consumer);

    let result: agent_client_protocol::Result<PromptResponse> = js_request(
        &js,
        "acp.session.s1.agent.prompt",
        &agent_client_protocol::PromptRequest::new("s1", vec![]),
        &trogon_std::FailNextSerialize::new(1),
        &test_prefix(),
        &test_sid("s1"),
        &ReqId::from_test("req-1"),
        Duration::from_secs(5),
    )
    .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("serialize"));
}
