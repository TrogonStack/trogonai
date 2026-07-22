use super::*;
use crate::agent::Bridge;
use crate::config::Config;
use agent_client_protocol::schema::v1::StopReason;
use trogon_nats::MockNatsClient;
use trogon_std::time::MockClock;

fn make_bridge() -> Bridge<MockNatsClient, MockClock, crate::agent::test_support::MockJs> {
    Bridge::new(
        MockNatsClient::new(),
        crate::agent::test_support::MockJs::new(),
        MockClock::new(),
        &opentelemetry::global::meter("acp-nats-test"),
        Config::for_test("acp"),
        tokio::sync::mpsc::channel(1).0,
    )
}

fn response_with_prompt_id(
    stop_reason: StopReason,
    prompt_token: PromptToken,
) -> (async_nats::header::HeaderMap, Vec<u8>) {
    let mut meta = serde_json::Map::new();
    meta.insert("prompt_id".to_string(), serde_json::json!(prompt_token.0));
    let response = PromptResponse::new(stop_reason).meta(meta);
    crate::client::test_support::encode_wire_notification("ext/session/prompt_response", &response)
}

#[tokio::test]
async fn resolves_waiter() {
    let bridge = make_bridge();
    let session_id: SessionId = "prompt-resp-001".into();

    let (rx, token) = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone())
        .unwrap();

    let (headers, payload) = response_with_prompt_id(StopReason::EndTurn, token);

    handle("prompt-resp-001", &headers, &payload, None, &bridge).await;

    let result = rx
        .await
        .expect("Should receive response")
        .expect("Prompt response should not include error");
    assert_eq!(result.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn no_waiter_does_not_panic() {
    let bridge = make_bridge();
    let (headers, payload) = response_with_prompt_id(StopReason::EndTurn, PromptToken(0));

    handle("no-waiter-session", &headers, &payload, None, &bridge).await;
}

#[tokio::test]
async fn invalid_payload_with_prompt_id_forwards_parse_error() {
    let bridge = make_bridge();
    let session_id: SessionId = "bad-payload-001".into();

    let (rx, token) = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone())
        .unwrap();

    let payload = format!(r#"{{"meta":{{"prompt_id":{}}},"stop_reason":"invalid"}}"#, token.0);

    handle(
        "bad-payload-001",
        &async_nats::header::HeaderMap::new(),
        payload.as_bytes(),
        None,
        &bridge,
    )
    .await;

    let result = rx
        .await
        .expect("Should receive resolved parse error")
        .expect_err("Parse failure should be forwarded to waiter");
    assert!(!result.is_empty(), "Expected parse error to be forwarded");
}

#[tokio::test]
async fn missing_prompt_id_is_rejected() {
    let bridge = make_bridge();
    let session_id: SessionId = "no-token-session".into();

    let (rx, _) = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone())
        .unwrap();

    let response = PromptResponse::new(StopReason::EndTurn);
    let (headers, payload) =
        crate::client::test_support::encode_wire_notification("ext/session/prompt_response", &response);

    handle("no-token-session", &headers, &payload, None, &bridge).await;

    assert!(
        bridge.pending_session_prompt_responses.has_waiter(&session_id),
        "waiter should remain when response lacks prompt_id"
    );
    bridge
        .pending_session_prompt_responses
        .remove_waiter_for_test(&session_id);
    drop(rx);
}

#[tokio::test]
async fn invalid_session_id_is_rejected() {
    let bridge = make_bridge();
    let session_id: SessionId = "valid-session".into();

    let (rx, token) = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone())
        .unwrap();

    let (headers, payload) = response_with_prompt_id(StopReason::EndTurn, token);

    handle("session.with.dots", &headers, &payload, None, &bridge).await;
    handle("session*wild", &headers, &payload, None, &bridge).await;
    handle("session id", &headers, &payload, None, &bridge).await;

    assert!(
        bridge.pending_session_prompt_responses.has_waiter(&session_id),
        "invalid session IDs should not resolve valid waiter",
    );

    bridge
        .pending_session_prompt_responses
        .remove_waiter_for_test(&session_id);
    assert!(
        !bridge.pending_session_prompt_responses.has_waiter(&session_id),
        "waiter should be removed"
    );
    drop(rx);
}

#[tokio::test]
async fn late_response_with_wrong_token_does_not_resolve_new_prompt() {
    let bridge = make_bridge();
    let session_id: SessionId = "same-session".into();

    let (_rx1, token1) = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone())
        .unwrap();
    bridge
        .pending_session_prompt_responses
        .resolve_waiter(&session_id, token1, Ok(PromptResponse::new(StopReason::EndTurn)))
        .unwrap();
    let _ = _rx1.await;

    let (rx2, token2) = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone())
        .unwrap();

    let (late_headers, late_payload) = response_with_prompt_id(StopReason::EndTurn, token1);
    handle("same-session", &late_headers, &late_payload, None, &bridge).await;

    assert!(
        bridge.pending_session_prompt_responses.has_waiter(&session_id),
        "late response with old token must not resolve new prompt"
    );
    bridge
        .pending_session_prompt_responses
        .resolve_waiter(&session_id, token2, Ok(PromptResponse::new(StopReason::EndTurn)))
        .unwrap();
    let result = rx2.await.unwrap().unwrap();
    assert_eq!(result.stop_reason, StopReason::EndTurn);
}
