use super::*;
use crate::config::Config;
use agent_client_protocol::v1::schema::StopReason;
use jsonrpc_nats::{Message, encode};
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::mocks::*;

fn make_nats_msg(payload: &[u8], headers: Option<async_nats::HeaderMap>) -> async_nats::Message {
    async_nats::Message {
        subject: "test".into(),
        reply: None,
        payload: bytes::Bytes::from(payload.to_vec()),
        headers,
        status: None,
        description: None,
        length: payload.len(),
    }
}

use crate::agent::test_support::{
    MockJs, has_error_metric, has_request_metric, mock_bridge_with_metrics, reply_when_published,
};

type RespTx = futures::channel::mpsc::UnboundedSender<Result<MockJsMessage, trogon_nats::mocks::MockError>>;

fn reply_success_when_published<Res: serde::Serialize>(js: &MockJs, tx: RespTx, result: &Res) {
    let result = serde_json::to_value(result).unwrap();
    reply_when_published(&js.publisher, tx, move |request_headers| {
        let encoded = encode(&Message::Success {
            id: crate::wire::response_id_from_request_headers(&request_headers),
            result,
        })
        .unwrap();
        (encoded.headers, encoded.body)
    });
}

fn reply_error_when_published(js: &MockJs, tx: RespTx, error: &Error) {
    let (code, message, data) = (i32::from(error.code), error.message.clone(), error.data.clone());
    reply_when_published(&js.publisher, tx, move |request_headers| {
        let encoded = encode(&Message::Error {
            id: crate::wire::response_id_from_request_headers(&request_headers),
            code,
            message,
            data,
        })
        .unwrap();
        (encoded.headers, encoded.body)
    });
}

fn reply_raw_when_published(js: &MockJs, tx: RespTx, payload: &'static [u8]) {
    reply_when_published(&js.publisher, tx, move |request_headers| {
        let mut headers = async_nats::HeaderMap::new();
        if let Some(literal) = request_headers.get(jsonrpc_nats::HEADER_ID) {
            headers.insert(jsonrpc_nats::HEADER_ID, literal.as_str());
        }
        (headers, bytes::Bytes::from_static(payload))
    });
}

fn mock_bridge() -> (
    AdvancedMockNatsClient,
    MockJs,
    Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock, MockJs>,
) {
    let mock = AdvancedMockNatsClient::new();
    let js = MockJs::new();
    let bridge = Bridge::new(
        mock.clone(),
        js.clone(),
        trogon_std::time::SystemClock,
        &opentelemetry::global::meter("prompt-js-test"),
        Config::for_test("acp").with_prompt_timeout(std::time::Duration::from_secs(5)),
    );
    (mock, js, bridge)
}

#[tokio::test]
async fn prompt_rejects_invalid_session_id() {
    let (_mock, _js, bridge) = mock_bridge();
    let err = handle(&bridge, PromptRequest::new("invalid.session.id", vec![]))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn prompt_js_success() {
    let (mock, js, bridge) = mock_bridge();

    // cancel sub for core NATS
    let _cancel_tx = mock.inject_messages();

    // response consumer
    let (resp_consumer, resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(resp_consumer);

    reply_success_when_published(&js, resp_tx, &PromptResponse::new(StopReason::EndTurn));

    let result = handle(&bridge, PromptRequest::new("s1", vec![])).await;

    let response = result.expect("expected Ok prompt response");
    assert_eq!(response.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn prompt_js_cancelled_runner_response() {
    let (_mock, js, bridge) = mock_bridge();

    let (resp_consumer, resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(resp_consumer);
    reply_success_when_published(&js, resp_tx, &PromptResponse::new(StopReason::Cancelled));

    let result = handle(&bridge, PromptRequest::new("s1", vec![])).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().stop_reason, StopReason::Cancelled);
}

#[tokio::test]
async fn prompt_js_timeout() {
    let mock = AdvancedMockNatsClient::new();
    let js = MockJs::new();
    let bridge = Bridge::new(
        mock.clone(),
        js.clone(),
        trogon_std::time::SystemClock,
        &opentelemetry::global::meter("prompt-js-timeout-test"),
        Config::for_test("acp").with_prompt_timeout(std::time::Duration::from_millis(50)),
    );

    let _cancel_tx = mock.inject_messages();

    let (resp_consumer, _resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(resp_consumer);

    let result = handle(&bridge, PromptRequest::new("s1", vec![])).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("timed out"));
}

#[tokio::test]
async fn prompt_js_publish_failure() {
    let (mock, js, bridge) = mock_bridge();
    let _cancel_tx = mock.inject_messages();

    let (resp_consumer, _resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(resp_consumer);

    js.publisher.fail_next_js_publish();

    let result = handle(&bridge, PromptRequest::new("s1", vec![])).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("js publish"));
}

#[tokio::test]
async fn prompt_js_bad_response_payload() {
    let (mock, js, bridge) = mock_bridge();
    let _cancel_tx = mock.inject_messages();

    let (resp_consumer, resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(resp_consumer);

    reply_raw_when_published(&js, resp_tx, b"not json");

    let result = handle(&bridge, PromptRequest::new("s1", vec![])).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("bad response payload"));
}

#[tokio::test]
async fn prompt_terminates_invalid_results_and_response_metadata() -> Result<(), Box<dyn std::error::Error>> {
    for (case, mut body, error_projection) in [
        (
            "invalid result",
            serde_json::json!({"jsonrpc": "2.0", "result": null}),
            None,
        ),
        (
            "request on the response stream",
            serde_json::json!({"jsonrpc": "2.0", "method": "session/prompt", "params": {}}),
            None,
        ),
        (
            "conflicting error-code projection",
            serde_json::json!({"jsonrpc": "2.0", "result": {"stopReason": "end_turn"}}),
            Some("-32603"),
        ),
    ] {
        let (mock, js, bridge, exporter, provider) = mock_bridge_with_metrics();
        let _cancel_tx = mock.inject_messages();
        let (consumer, tx) = MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(consumer);

        let reply = async {
            let request = loop {
                if let Some(request) = js
                    .publisher
                    .published_messages()
                    .into_iter()
                    .find(|message| message.headers.get(jsonrpc_nats::HEADER_ID).is_some())
                {
                    break request;
                }
                tokio::task::yield_now().await;
            };
            let literal = request
                .headers
                .get(jsonrpc_nats::HEADER_ID)
                .ok_or("published request has no id")?;
            body["id"] = serde_json::from_str(literal.as_str())?;
            let mut headers = async_nats::HeaderMap::new();
            headers.insert(jsonrpc_nats::HEADER_ID, literal.as_str());
            if let Some(code) = error_projection {
                headers.insert(jsonrpc_nats::HEADER_ERROR_CODE, code);
            }
            let message = MockJsMessage::new(make_nats_msg(&serde_json::to_vec(&body)?, Some(headers)));
            let observed = message.clone();
            assert!(tx.unbounded_send(Ok(message)).is_ok());
            Ok::<_, Box<dyn std::error::Error>>(observed)
        };
        let request = PromptRequest::new(
            "s1",
            vec![agent_client_protocol::v1::schema::ContentBlock::from("Continue.")],
        );
        let prompt = crate::AgentHandler::prompt(&bridge, request);
        tokio::pin!(prompt);
        let observed = tokio::select! {
            _result = &mut prompt => return Err("prompt completed before the correlated response".into()),
            observed = reply => observed?,
        };
        let error = prompt.await.err().ok_or("invalid response was accepted")?;

        assert_eq!(error.code, ErrorCode::INTERNAL_ERROR, "{case}");
        assert_eq!(error.message, "bad response payload", "{case}");
        assert_eq!(
            observed.signals(),
            vec![AckKindSnapshot::AckWith(AckKindValue::Term)],
            "{case}"
        );
        provider.force_flush()?;
        let metrics = exporter.get_finished_metrics()?;
        assert!(has_error_metric(&metrics, "prompt", "bad_response_payload"), "{case}");
        assert!(has_request_metric(&metrics, "prompt", false), "{case}");
    }
    Ok(())
}

#[tokio::test]
async fn prompt_js_agent_error_response() {
    let (mock, js, bridge) = mock_bridge();
    let _cancel_tx = mock.inject_messages();

    let (resp_consumer, resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(resp_consumer);

    let agent_err = Error::new(ErrorCode::INTERNAL_ERROR, "agent blew up");
    reply_error_when_published(&js, resp_tx, &agent_err);

    let result = handle(&bridge, PromptRequest::new("s1", vec![])).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, ErrorCode::INTERNAL_ERROR);
    assert!(err.message.contains("agent blew up"));
}

#[tokio::test(start_paused = true)]
async fn prompt_js_response_stream_closed() {
    let (mock, js, bridge) = mock_bridge();
    let _cancel_tx = mock.inject_messages();

    let (resp_consumer, resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(resp_consumer);

    drop(resp_tx);

    let result = handle(&bridge, PromptRequest::new("s1", vec![])).await;
    let message = result.unwrap_err().message;
    assert!(message.contains("prompt timed out waiting for runner"), "{message}");
}

#[tokio::test]
async fn prompt_js_get_resp_stream_failure() {
    let (mock, js, bridge) = mock_bridge();
    let _cancel_tx = mock.inject_messages();
    js.consumer_factory.fail_get_stream_at(1);
    let result = handle(&bridge, PromptRequest::new("s1", vec![])).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("get responses stream"));
}

#[tokio::test]
async fn prompt_js_resp_consumer_creation_failure() {
    let (mock, _js, bridge) = mock_bridge();
    let _cancel_tx = mock.inject_messages();
    // Add no consumers — create_consumer fails
    let result = handle(&bridge, PromptRequest::new("s1", vec![])).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("create response consumer"));
}

#[tokio::test]
async fn prompt_does_not_require_a_cancellation_broadcast_subscription() {
    let (mock, js, bridge) = mock_bridge();
    let (resp_consumer, resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(resp_consumer);
    reply_success_when_published(&js, resp_tx, &PromptResponse::new(StopReason::EndTurn));
    let result = handle(&bridge, PromptRequest::new("s1", vec![])).await;
    assert_eq!(result.unwrap().stop_reason, StopReason::EndTurn);
    assert!(mock.subscribed_to().is_empty());
}

#[tokio::test]
async fn prompt_js_resp_messages_failure() {
    let (mock, js, bridge) = mock_bridge();
    let _cancel_tx = mock.inject_messages();

    let failing_consumer = trogon_nats::jetstream::MockJetStreamConsumer::failing();
    js.consumer_factory.add_consumer(failing_consumer);

    let result = handle(&bridge, PromptRequest::new("s1", vec![])).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("response messages"));
}

#[tokio::test(start_paused = true)]
async fn prompt_js_response_consumer_error() {
    let (mock, js, bridge) = mock_bridge();
    let _cancel_tx = mock.inject_messages();

    let (resp_consumer, resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(resp_consumer);

    resp_tx
        .unbounded_send(Err(trogon_nats::mocks::MockError("consumer error".to_string())))
        .unwrap();

    let result = handle(&bridge, PromptRequest::new("s1", vec![])).await;
    let message = result.unwrap_err().message;
    assert!(message.contains("prompt timed out waiting for runner"), "{message}");
    assert!(message.contains("last response consumer error"), "{message}");
}

/// Regression: under a network stall the client got `-32603 "response consumer:
/// missed idle heartbeat"` while the runner finished the turn; its stop reason
/// never reached the client.
#[tokio::test(start_paused = true)]
async fn prompt_returns_the_real_stop_reason_after_a_missed_heartbeat() {
    let (mock, js, bridge) = mock_bridge();
    let _cancel_tx = mock.inject_messages();

    let (stalled, stalled_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    let (recreated, recreated_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(stalled);
    js.consumer_factory.add_consumer(recreated);

    stalled_tx
        .unbounded_send(Err(trogon_nats::mocks::MockError("missed idle heartbeat".to_string())))
        .unwrap();
    reply_success_when_published(&js, recreated_tx, &PromptResponse::new(StopReason::EndTurn));

    let result = handle(&bridge, PromptRequest::new("s1", vec![])).await;

    assert_eq!(
        result
            .expect("the stop reason survives the consumer failure")
            .stop_reason,
        StopReason::EndTurn
    );
}

#[tokio::test]
async fn prompt_js_skips_a_reply_belonging_to_another_request_on_the_session() {
    // The response consumer is session-scoped, so a second prompt in flight on
    // the same session lands on it too. Matching on `Jsonrpc-Id` is what keeps
    // one prompt from resolving with another's answer.
    let (mock, js, bridge) = mock_bridge();

    let _cancel_tx = mock.inject_messages();

    let (resp_consumer, resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(resp_consumer);

    let foreign = encode(&Message::Success {
        id: jsonrpc_nats::ResponseId::String("some-other-prompt".into()),
        result: serde_json::to_value(PromptResponse::new(StopReason::Cancelled)).unwrap(),
    })
    .unwrap();
    resp_tx
        .unbounded_send(Ok(MockJsMessage::new(make_nats_msg(
            &foreign.body,
            Some(foreign.headers),
        ))))
        .unwrap();

    reply_success_when_published(&js, resp_tx, &PromptResponse::new(StopReason::EndTurn));

    let result = handle(&bridge, PromptRequest::new("s1", vec![])).await;

    assert_eq!(
        result.expect("expected Ok prompt response").stop_reason,
        StopReason::EndTurn
    );
}
