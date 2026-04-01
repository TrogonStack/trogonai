use agent_client_protocol::{
    Error, ErrorCode, PromptRequest, PromptResponse, SessionNotification, StopReason,
};
use async_nats::jetstream::AckKind;
use bytes::Bytes;
use futures::StreamExt;
use tokio::time::timeout;
use tracing::{instrument, warn};
use trogon_nats::jetstream::{
    JetStreamConsumer as _, JetStreamCreateConsumer as _, JetStreamGetStream, JetStreamPublisher,
    JsAck as _, JsAckWith as _, JsMessageRef as _, JsRequestMessage,
};
use trogon_std::JsonSerialize;

use crate::agent::Bridge;
use crate::constants::SESSION_ID_HEADER;
use crate::jetstream::{consumers, streams};
use crate::nats::{FlushClient, PublishClient, RequestClient, SubscribeClient, session};
use crate::session_id::AcpSessionId;

pub use trogon_nats::REQ_ID_HEADER;

#[instrument(
    name = "acp.session.prompt",
    skip(bridge, args, serializer),
    fields(session_id = %args.session_id)
)]
pub async fn handle<N, C, J, S>(
    bridge: &Bridge<N, C, J>,
    args: PromptRequest,
    serializer: &S,
) -> agent_client_protocol::Result<PromptResponse>
where
    N: RequestClient + PublishClient + SubscribeClient + FlushClient,
    C: trogon_std::time::GetElapsed,
    J: JetStreamPublisher + JetStreamGetStream,
    <<J::Stream as trogon_nats::jetstream::JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Message: JsRequestMessage,
    S: JsonSerialize,
{
    let start = bridge.clock.now();

    let session_id = AcpSessionId::try_from(&args.session_id).map_err(|_| {
        bridge.metrics.record_error("prompt", "invalid_session_id");
        Error::new(ErrorCode::InvalidParams.into(), "Invalid session ID")
    })?;

    let req_id = uuid::Uuid::new_v4().to_string();
    let prefix = bridge.config.acp_prefix_ref();

    let result = handle_js(
        bridge,
        bridge.js(),
        &args,
        serializer,
        &session_id,
        prefix,
        &req_id,
    )
    .await;

    bridge.metrics.record_request(
        "prompt",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

async fn handle_js<N, C, J, S>(
    bridge: &Bridge<N, C, J>,
    js: &J,
    args: &PromptRequest,
    serializer: &S,
    session_id: &AcpSessionId,
    prefix: &crate::acp_prefix::AcpPrefix,
    req_id: &str,
) -> agent_client_protocol::Result<PromptResponse>
where
    N: SubscribeClient,
    C: trogon_std::time::GetElapsed,
    J: JetStreamPublisher + JetStreamGetStream,
    <<J::Stream as trogon_nats::jetstream::JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Message: JsRequestMessage,
    S: JsonSerialize,
{
    // Create consumers BEFORE publishing — same principle as subscribe-before-publish.
    // JetStream consumers with DeliverAll replay from stream start, so they'll see the
    // response even if the runner responds before we start consuming.
    let sid = session_id.as_str();
    let notifications_stream = streams::notifications_stream_name(prefix);
    let notif_config = consumers::prompt_notifications_consumer(prefix, sid, req_id);
    let notif_stream = js.get_stream(&notifications_stream).await.map_err(|e| {
        Error::new(
            ErrorCode::InternalError.into(),
            format!("get notifications stream: {e}"),
        )
    })?;
    let notif_consumer = notif_stream
        .create_consumer(notif_config)
        .await
        .map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("create notification consumer: {e}"),
            )
        })?;
    let mut notif_messages = notif_consumer.messages().await.map_err(|e| {
        Error::new(
            ErrorCode::InternalError.into(),
            format!("notification messages: {e}"),
        )
    })?;

    let responses_stream = streams::responses_stream_name(prefix);
    let resp_config = consumers::prompt_response_consumer(prefix, sid, req_id);
    let resp_stream = js.get_stream(&responses_stream).await.map_err(|e| {
        Error::new(
            ErrorCode::InternalError.into(),
            format!("get responses stream: {e}"),
        )
    })?;
    let resp_consumer = resp_stream
        .create_consumer(resp_config)
        .await
        .map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("create response consumer: {e}"),
            )
        })?;
    let mut resp_messages = resp_consumer.messages().await.map_err(|e| {
        Error::new(
            ErrorCode::InternalError.into(),
            format!("response messages: {e}"),
        )
    })?;

    // Cancel still uses core NATS — it's a fire-and-forget signal, not persisted.
    let mut cancel_sub = bridge
        .nats
        .subscribe(session::agent::CancelledSubject::new(prefix, session_id))
        .await
        .map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("subscribe cancelled: {e}"),
            )
        })?;

    // Now publish — consumers are ready, no race condition.
    let payload_bytes = serializer
        .to_vec(args)
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("serialize: {e}")))?;

    let mut headers = async_nats::HeaderMap::new();
    headers.insert(REQ_ID_HEADER, req_id);
    headers.insert(SESSION_ID_HEADER, sid);

    let prompt_subject = session::agent::PromptSubject::new(prefix, session_id);
    js.publish_with_headers(prompt_subject, headers, Bytes::from(payload_bytes))
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("js publish: {e}")))?
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("js ack: {e}")))?;

    let op_timeout = bridge.config.prompt_timeout();

    loop {
        tokio::select! {
            notif = notif_messages.next() => {
                match notif {
                    None => {
                        bridge.metrics.record_error("prompt", "notification_stream_closed");
                        break Err(Error::new(
                            ErrorCode::InternalError.into(),
                            "notification stream closed unexpectedly",
                        ));
                    }
                    Some(Err(e)) => {
                        bridge.metrics.record_error("prompt", "notification_consumer_error");
                        break Err(Error::new(
                            ErrorCode::InternalError.into(),
                            format!("notification consumer: {e}"),
                        ));
                    }
                    Some(Ok(js_msg)) => {
                        let notification: SessionNotification = match serde_json::from_slice(js_msg.message().payload.as_ref()) {
                            Ok(n) => n,
                            Err(e) => {
                                warn!(error = %e, "bad notification payload; skipping");
                                let _ = js_msg.ack().await;
                                continue;
                            }
                        };
                        let _ = js_msg.ack().await;
                        let _ = bridge.notification_sender.send(notification).await.inspect_err(|_| {
                            warn!("notification receiver dropped; continuing prompt");
                        });
                    }
                }
            }
            resp = timeout(op_timeout, resp_messages.next()) => {
                match resp {
                    Ok(Some(Ok(js_msg))) => {
                        match serde_json::from_slice::<PromptResponse>(js_msg.message().payload.as_ref()) {
                            Ok(response) => {
                                let _ = js_msg.ack().await;
                                break Ok(response);
                            }
                            Err(_) => {
                                if let Ok(agent_err) = serde_json::from_slice::<Error>(js_msg.message().payload.as_ref()) {
                                    let _ = js_msg.ack().await;
                                    break Err(agent_err);
                                }
                                let _ = js_msg.ack_with(AckKind::Term).await;
                                bridge.metrics.record_error("prompt", "bad_response_payload");
                                break Err(Error::new(
                                    ErrorCode::InternalError.into(),
                                    "bad response payload",
                                ));
                            }
                        }
                    }
                    Ok(Some(Err(e))) => {
                        bridge.metrics.record_error("prompt", "response_consumer_error");
                        break Err(Error::new(
                            ErrorCode::InternalError.into(),
                            format!("response consumer: {e}"),
                        ));
                    }
                    Ok(None) => {
                        bridge.metrics.record_error("prompt", "response_stream_closed");
                        break Err(Error::new(
                            ErrorCode::InternalError.into(),
                            "response stream closed unexpectedly",
                        ));
                    }
                    Err(_elapsed) => {
                        bridge.metrics.record_error("prompt", "prompt_timeout");
                        break Err(Error::new(
                            ErrorCode::InternalError.into(),
                            "prompt timed out waiting for runner",
                        ));
                    }
                }
            }
            _ = cancel_sub.next() => {
                break Ok(PromptResponse::new(StopReason::Cancelled));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use trogon_nats::AdvancedMockNatsClient;

    fn make_nats_msg(payload: &[u8]) -> async_nats::Message {
        async_nats::Message {
            subject: "test".into(),
            reply: None,
            payload: bytes::Bytes::from(payload.to_vec()),
            headers: None,
            status: None,
            description: None,
            length: payload.len(),
        }
    }

    use crate::agent::test_support::MockJs;

    fn mock_bridge() -> (
        AdvancedMockNatsClient,
        MockJs,
        Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock, MockJs>,
    ) {
        let mock = AdvancedMockNatsClient::new();
        let js = MockJs::new();
        let (notification_tx, _notification_rx) =
            tokio::sync::mpsc::channel::<SessionNotification>(64);
        let bridge = Bridge::new(
            mock.clone(),
            js.clone(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("prompt-js-test"),
            Config::for_test("acp").with_prompt_timeout(std::time::Duration::from_secs(5)),
            notification_tx,
        );
        (mock, js, bridge)
    }

    #[tokio::test]
    async fn prompt_rejects_invalid_session_id() {
        let (_mock, _js, bridge) = mock_bridge();
        let err = handle(
            &bridge,
            PromptRequest::new("invalid.session.id", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn prompt_js_success() {
        let (mock, js, bridge) = mock_bridge();

        // cancel sub for core NATS
        let _cancel_tx = mock.inject_messages();

        // notification consumer
        let (notif_consumer, notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);

        // response consumer
        let (resp_consumer, resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(resp_consumer);

        let response = PromptResponse::new(StopReason::EndTurn);
        let msg = trogon_nats::jetstream::MockJsMessage::new(make_nats_msg(
            &serde_json::to_vec(&response).unwrap(),
        ));
        resp_tx.unbounded_send(Ok(msg)).unwrap();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;

        drop(notif_tx);
        let response = result.expect("expected Ok prompt response");
        assert_eq!(response.stop_reason, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn prompt_js_cancel() {
        let (mock, js, bridge) = mock_bridge();

        let cancel_tx = mock.inject_messages();

        let (notif_consumer, _notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);

        let (resp_consumer, _resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(resp_consumer);

        cancel_tx.unbounded_send(make_nats_msg(b"")).unwrap();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().stop_reason, StopReason::Cancelled);
    }

    #[tokio::test]
    async fn prompt_js_timeout() {
        let mock = AdvancedMockNatsClient::new();
        let js = MockJs::new();
        let (notification_tx, _notification_rx) =
            tokio::sync::mpsc::channel::<SessionNotification>(64);
        let bridge = Bridge::new(
            mock.clone(),
            js.clone(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("prompt-js-timeout-test"),
            Config::for_test("acp").with_prompt_timeout(std::time::Duration::from_millis(50)),
            notification_tx,
        );

        let _cancel_tx = mock.inject_messages();

        let (notif_consumer, _notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);

        let (resp_consumer, _resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(resp_consumer);

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("timed out"));
    }

    #[tokio::test]
    async fn prompt_js_notification_forwarding() {
        let mock = AdvancedMockNatsClient::new();
        let js = MockJs::new();
        let (notification_tx, _notification_rx) =
            tokio::sync::mpsc::channel::<SessionNotification>(64);
        let bridge = Bridge::new(
            mock.clone(),
            js.clone(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("prompt-js-notif-test"),
            Config::for_test("acp").with_prompt_timeout(std::time::Duration::from_secs(5)),
            notification_tx,
        );

        let _cancel_tx = mock.inject_messages();

        let (notif_consumer, notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);

        let (resp_consumer, resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(resp_consumer);

        let notification = SessionNotification::new(
            "s1",
            agent_client_protocol::SessionUpdate::AgentThoughtChunk(
                agent_client_protocol::ContentChunk::new(
                    agent_client_protocol::ContentBlock::Text(
                        agent_client_protocol::TextContent::new("thinking..."),
                    ),
                ),
            ),
        );
        let notif_msg = trogon_nats::jetstream::MockJsMessage::new(make_nats_msg(
            &serde_json::to_vec(&notification).unwrap(),
        ));
        notif_tx.unbounded_send(Ok(notif_msg)).unwrap();

        let response = PromptResponse::new(StopReason::EndTurn);
        let resp_msg = trogon_nats::jetstream::MockJsMessage::new(make_nats_msg(
            &serde_json::to_vec(&response).unwrap(),
        ));
        resp_tx.unbounded_send(Ok(resp_msg)).unwrap();
        let _notif_keeper = notif_tx;

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;

        let response = result.expect("expected Ok prompt response");
        assert_eq!(response.stop_reason, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn prompt_js_publish_failure() {
        let (mock, js, bridge) = mock_bridge();
        let _cancel_tx = mock.inject_messages();

        let (notif_consumer, _notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);
        let (resp_consumer, _resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(resp_consumer);

        js.publisher.fail_next_js_publish();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("js publish"));
    }

    #[tokio::test]
    async fn prompt_js_bad_response_payload() {
        let (mock, js, bridge) = mock_bridge();
        let _cancel_tx = mock.inject_messages();

        let (notif_consumer, _notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);

        let (resp_consumer, resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(resp_consumer);

        let msg = trogon_nats::jetstream::MockJsMessage::new(make_nats_msg(b"not json"));
        resp_tx.unbounded_send(Ok(msg)).unwrap();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("bad response payload"));
    }

    #[tokio::test]
    async fn prompt_js_agent_error_response() {
        let (mock, js, bridge) = mock_bridge();
        let _cancel_tx = mock.inject_messages();

        let (notif_consumer, _notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);

        let (resp_consumer, resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(resp_consumer);

        let agent_err = Error::new(ErrorCode::InternalError.into(), "agent blew up");
        let msg = trogon_nats::jetstream::MockJsMessage::new(make_nats_msg(
            &serde_json::to_vec(&agent_err).unwrap(),
        ));
        resp_tx.unbounded_send(Ok(msg)).unwrap();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::InternalError);
        assert!(err.message.contains("agent blew up"));
    }

    #[tokio::test]
    async fn prompt_js_response_stream_closed() {
        let (mock, js, bridge) = mock_bridge();
        let _cancel_tx = mock.inject_messages();

        let (notif_consumer, _notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);

        let (resp_consumer, resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(resp_consumer);

        drop(resp_tx);

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn prompt_js_get_notif_stream_failure() {
        let (mock, js, bridge) = mock_bridge();
        let _cancel_tx = mock.inject_messages();
        js.consumer_factory.fail_get_stream_at(1);
        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .message
                .contains("get notifications stream")
        );
    }

    #[tokio::test]
    async fn prompt_js_get_resp_stream_failure() {
        let (mock, js, bridge) = mock_bridge();
        let _cancel_tx = mock.inject_messages();
        let (notif_consumer, _notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);
        js.consumer_factory.fail_get_stream_at(2);
        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("get responses stream"));
    }

    #[tokio::test]
    async fn prompt_js_notif_consumer_creation_failure() {
        let (mock, _js, bridge) = mock_bridge();
        let _cancel_tx = mock.inject_messages();
        // Don't add any consumers — first create_consumer call will fail
        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .message
                .contains("create notification consumer")
        );
    }

    #[tokio::test]
    async fn prompt_js_resp_consumer_creation_failure() {
        let (mock, js, bridge) = mock_bridge();
        let _cancel_tx = mock.inject_messages();
        // Add notif consumer but not response consumer
        let (notif_consumer, _notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);
        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .message
                .contains("create response consumer")
        );
    }

    #[tokio::test]
    async fn prompt_js_cancel_subscribe_failure() {
        let (_mock, js, bridge) = mock_bridge();
        // Don't inject cancel_tx — subscribe will fail (no streams in mock)
        let (notif_consumer, _notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);
        let (resp_consumer, _resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(resp_consumer);
        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("subscribe cancelled"));
    }

    #[tokio::test]
    async fn prompt_js_notif_messages_failure() {
        let (mock, js, bridge) = mock_bridge();
        let _cancel_tx = mock.inject_messages();

        let failing_consumer = trogon_nats::jetstream::MockJetStreamConsumer::failing();
        js.consumer_factory.add_consumer(failing_consumer);

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .message
                .contains("notification messages")
        );
    }

    #[tokio::test]
    async fn prompt_js_resp_messages_failure() {
        let (mock, js, bridge) = mock_bridge();
        let _cancel_tx = mock.inject_messages();

        let (notif_consumer, _notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);

        let failing_consumer = trogon_nats::jetstream::MockJetStreamConsumer::failing();
        js.consumer_factory.add_consumer(failing_consumer);

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("response messages"));
    }

    #[tokio::test]
    async fn prompt_js_notification_consumer_error() {
        let (mock, js, bridge) = mock_bridge();
        let _cancel_tx = mock.inject_messages();

        let (notif_consumer, notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);

        let (resp_consumer, _resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(resp_consumer);

        notif_tx
            .unbounded_send(Err(trogon_nats::mocks::MockError(
                "consumer error".to_string(),
            )))
            .unwrap();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .message
                .contains("notification consumer")
        );
    }

    #[tokio::test]
    async fn prompt_js_response_consumer_error() {
        let (mock, js, bridge) = mock_bridge();
        let _cancel_tx = mock.inject_messages();

        let (notif_consumer, _notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);

        let (resp_consumer, resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(resp_consumer);

        resp_tx
            .unbounded_send(Err(trogon_nats::mocks::MockError(
                "consumer error".to_string(),
            )))
            .unwrap();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("response consumer"));
    }

    #[tokio::test]
    async fn prompt_js_bad_notification_payload_skipped() {
        let mock = AdvancedMockNatsClient::new();
        let js = MockJs::new();
        let (notification_tx, _notification_rx) =
            tokio::sync::mpsc::channel::<SessionNotification>(64);
        let bridge = Bridge::new(
            mock.clone(),
            js.clone(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("prompt-js-bad-notif-test"),
            Config::for_test("acp").with_prompt_timeout(std::time::Duration::from_millis(100)),
            notification_tx,
        );
        let _cancel_tx = mock.inject_messages();

        let (notif_consumer, notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);

        let (resp_consumer, _resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(resp_consumer);

        // Send only bad notification, no response. Handler processes bad notif
        // (warn, ack, continue), then times out waiting for response.
        let bad_notif = trogon_nats::jetstream::MockJsMessage::new(make_nats_msg(b"not json"));
        notif_tx.unbounded_send(Ok(bad_notif)).unwrap();
        let _notif_keeper = notif_tx;

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        // Times out after processing bad notification
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("timed out"));
    }

    #[tokio::test]
    async fn prompt_js_notification_receiver_dropped() {
        use tracing_subscriber::util::SubscriberInitExt;
        let _guard = tracing_subscriber::fmt().with_test_writer().set_default();

        let mock = AdvancedMockNatsClient::new();
        let js = MockJs::new();
        let (notification_tx, notification_rx) =
            tokio::sync::mpsc::channel::<SessionNotification>(64);
        drop(notification_rx);
        let bridge = Bridge::new(
            mock.clone(),
            js.clone(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("prompt-js-rx-dropped-test"),
            Config::for_test("acp").with_prompt_timeout(std::time::Duration::from_millis(100)),
            notification_tx,
        );

        let _cancel_tx = mock.inject_messages();

        let (notif_consumer, notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);
        let (resp_consumer, _resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(resp_consumer);

        let notification = SessionNotification::new(
            "s1",
            agent_client_protocol::SessionUpdate::AgentThoughtChunk(
                agent_client_protocol::ContentChunk::new(
                    agent_client_protocol::ContentBlock::Text(
                        agent_client_protocol::TextContent::new("thinking..."),
                    ),
                ),
            ),
        );
        let notif_msg = trogon_nats::jetstream::MockJsMessage::new(make_nats_msg(
            &serde_json::to_vec(&notification).unwrap(),
        ));
        notif_tx.unbounded_send(Ok(notif_msg)).unwrap();
        let _notif_keeper = notif_tx;

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("timed out"));
    }

    #[tokio::test]
    async fn prompt_js_notification_stream_closed() {
        let (mock, js, bridge) = mock_bridge();
        let _cancel_tx = mock.inject_messages();

        let (notif_consumer, notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);
        let (resp_consumer, _resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(resp_consumer);

        drop(notif_tx);

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .message
                .contains("notification stream closed")
        );
    }
}
