use crate::agent::Bridge;
use crate::config::Config;
use crate::tests::AdvancedMockNatsClient;
use crate::tests::noop_meter;
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification, ContentBlock,
    ExtNotification, ExtRequest, ExtResponse, InitializeRequest, InitializeResponse,
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest,
    ProtocolVersion, SetSessionModeRequest, SetSessionModeResponse, StopReason,
};
use serde_json::value::RawValue;
use std::time::Duration;
use trogon_std::time::MockClock;

fn mock_bridge() -> (
    AdvancedMockNatsClient,
    MockClock,
    Bridge<AdvancedMockNatsClient, MockClock>,
) {
    let mock = AdvancedMockNatsClient::new();
    let clock = MockClock::new();
    let bridge = Bridge::new(
        mock.clone(),
        clock.clone(),
        &noop_meter(),
        Config::for_test("acp"),
    );
    (mock, clock, bridge)
}

fn set_json_response<T: serde::Serialize>(mock: &AdvancedMockNatsClient, subject: &str, resp: &T) {
    let bytes = serde_json::to_vec(resp).unwrap();
    mock.set_response(subject, bytes.into());
}

// --- initialize ---

#[tokio::test]
async fn initialize_forwards_request_and_returns_response() {
    let (mock, _clock, bridge) = mock_bridge();
    let expected = InitializeResponse::new(ProtocolVersion::LATEST);
    set_json_response(&mock, "acp.agent.initialize", &expected);

    let request = InitializeRequest::new(ProtocolVersion::LATEST);
    let result = bridge.initialize(request).await;

    assert!(result.is_ok());
    let response = result.unwrap();
    assert_eq!(response.protocol_version, ProtocolVersion::LATEST);
}

#[tokio::test]
async fn initialize_returns_error_when_nats_request_fails() {
    let (mock, _clock, bridge) = mock_bridge();
    mock.fail_next_request();

    let request = InitializeRequest::new(ProtocolVersion::LATEST);
    let result = bridge.initialize(request).await;

    assert!(result.is_err());
}

// --- authenticate ---

#[tokio::test]
async fn authenticate_forwards_request_and_returns_response() {
    let (mock, _clock, bridge) = mock_bridge();
    let expected = AuthenticateResponse::new();
    set_json_response(&mock, "acp.agent.authenticate", &expected);

    let request = AuthenticateRequest::new("api-key");
    let result = bridge.authenticate(request).await;

    assert!(result.is_ok());
}

// --- new_session ---

#[tokio::test]
async fn new_session_forwards_request_and_returns_response() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (mock, _clock, bridge) = mock_bridge();
            let expected = NewSessionResponse::new("session-001");
            set_json_response(&mock, "acp.agent.session.new", &expected);

            let request = NewSessionRequest::new("/tmp");
            let result = bridge.new_session(request).await;

            assert!(result.is_ok());
            let response = result.unwrap();
            assert_eq!(response.session_id.to_string(), "session-001");
        })
        .await;
}

// --- load_session ---

#[tokio::test]
async fn load_session_forwards_request_and_returns_response() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (mock, _clock, bridge) = mock_bridge();
            let expected = LoadSessionResponse::new();
            set_json_response(&mock, "acp.session-load-001.agent.session.load", &expected);

            let request = LoadSessionRequest::new("session-load-001", "/tmp");
            let result = bridge.load_session(request).await;

            assert!(result.is_ok());
        })
        .await;
}

#[tokio::test]
async fn session_ready_publish_tasks_are_tracked_and_awaited() {
    let mock = AdvancedMockNatsClient::new();
    let request = NewSessionRequest::new("/tmp");
    let expected = NewSessionResponse::new("session-ready-track");
    set_json_response(&mock, "acp.agent.session.new", &expected);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let bridge = Bridge::new(
                mock.clone(),
                MockClock::new(),
                &noop_meter(),
                Config::for_test("acp"),
            );
            let result = bridge.new_session(request).await;
            assert!(result.is_ok());

            assert!(
                bridge.has_pending_session_ready_tasks(),
                "session.ready task should be tracked"
            );

            bridge.await_session_ready_tasks().await;

            assert!(
                !bridge.has_pending_session_ready_tasks(),
                "session.ready tasks should be fully awaited and cleared"
            );
        })
        .await;
}

// --- set_session_mode ---

#[tokio::test]
async fn set_session_mode_forwards_request_and_returns_response() {
    let (mock, _clock, bridge) = mock_bridge();
    let expected = SetSessionModeResponse::new();
    set_json_response(
        &mock,
        "acp.session-mode-001.agent.session.set_mode",
        &expected,
    );

    let request = SetSessionModeRequest::new("session-mode-001", "auto");
    let result = bridge.set_session_mode(request).await;

    assert!(result.is_ok());
}

// --- cancel ---

#[tokio::test]
async fn cancel_marks_session_as_cancelled() {
    let (_mock, _clock, bridge) = mock_bridge();
    let session_id = "cancel-session-001";

    assert!(
        bridge
            .cancelled_sessions
            .take_if_cancelled(&session_id.into(), &bridge.clock)
            .is_none()
    );

    let notification = CancelNotification::new(session_id);
    bridge.cancel(notification).await.unwrap();

    assert!(
        bridge
            .cancelled_sessions
            .take_if_cancelled(&session_id.into(), &bridge.clock)
            .is_some()
    );
}

#[tokio::test]
async fn cancel_resolves_pending_prompt_waiter_with_cancelled() {
    let (_mock, _clock, bridge) = mock_bridge();
    let session_id: agent_client_protocol::SessionId = "cancel-session-002".into();

    let rx = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone())
        .unwrap();

    let notification = CancelNotification::new(session_id.clone());
    bridge.cancel(notification).await.unwrap();

    let response = rx
        .await
        .expect("Should receive cancelled response")
        .expect("Prompt waiter should receive success response");
    assert_eq!(response.stop_reason, StopReason::Cancelled);
}

#[tokio::test]
async fn cancel_publishes_to_nats() {
    let (mock, _clock, bridge) = mock_bridge();
    let session_id = "cancel-session-003";

    let notification = CancelNotification::new(session_id);
    bridge.cancel(notification).await.unwrap();

    let published = mock.published_messages();
    assert!(
        published.iter().any(|s| s.contains("session.cancel")),
        "Expected cancel publish, got: {:?}",
        published
    );
}

#[tokio::test]
async fn cancel_session_evicts_expired_on_mark() {
    let (_mock, clock, bridge) = mock_bridge();

    let session_old: agent_client_protocol::SessionId = "old-session".into();
    let session_new: agent_client_protocol::SessionId = "new-session".into();

    // Mark the old session as cancelled at time 0
    bridge
        .cancelled_sessions
        .mark_cancelled(session_old.clone(), &bridge.clock);

    // Advance past the TTL so the old session expires
    clock.advance(Duration::from_secs(301));

    // Mark enough sessions to reach the periodic cancellation sweep threshold.
    // This preserves bounded cleanup behavior while avoiding O(n) on every call.
    for idx in 0..15 {
        let filler_session: agent_client_protocol::SessionId = format!("filler-{idx}").into();
        bridge
            .cancelled_sessions
            .mark_cancelled(filler_session, &bridge.clock);
    }

    // Mark a new session as cancelled; this call triggers the eviction sweep.
    bridge
        .cancelled_sessions
        .mark_cancelled(session_new.clone(), &bridge.clock);

    // The old session should have been evicted during mark_cancelled
    assert!(
        bridge
            .cancelled_sessions
            .take_if_cancelled(&session_old, &bridge.clock)
            .is_none()
    );

    // The new session should still be present
    assert!(
        bridge
            .cancelled_sessions
            .take_if_cancelled(&session_new, &bridge.clock)
            .is_some()
    );
}

// --- prompt ---

#[tokio::test]
async fn prompt_returns_cancelled_when_session_already_cancelled() {
    let (_mock, _clock, bridge) = mock_bridge();
    let session_id = "prompt-cancel-001";

    bridge
        .cancelled_sessions
        .mark_cancelled(session_id.into(), &bridge.clock);

    let request = PromptRequest::new(session_id, vec![ContentBlock::from("hello")]);
    let result = bridge.prompt(request).await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().stop_reason, StopReason::Cancelled);
}

#[tokio::test]
async fn prompt_rejects_duplicate_waiter_for_same_session() {
    let (_mock, _clock, bridge) = mock_bridge();
    let session_id: agent_client_protocol::SessionId = "prompt-duplicate-001".into();

    let _rx = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone())
        .unwrap();

    let request = PromptRequest::new(session_id, vec![ContentBlock::from("hello")]);
    let result = bridge.prompt(request).await;
    let error = result.expect_err("Second prompt should be rejected");
    assert!(
        error.message.contains("Duplicate prompt request"),
        "Unexpected error message: {}",
        error.message
    );
}

#[tokio::test]
async fn prompt_rejects_when_backpressure_is_exceeded() {
    let mock = AdvancedMockNatsClient::new();
    let clock = MockClock::new();
    let bridge = Bridge::new(
        mock,
        clock,
        &noop_meter(),
        Config::for_test("acp").with_max_concurrent_client_tasks(1),
    );

    assert!(bridge.try_acquire_prompt_slot());

    let request = PromptRequest::new("prompt-overflow", vec![ContentBlock::from("hello")]);
    let result = bridge.prompt(request).await;

    bridge.release_prompt_slot();

    let error = result.expect_err("Prompt should be rejected under backpressure");
    assert_eq!(
        error.message,
        "Bridge overloaded - too many concurrent prompt requests"
    );
}

// --- ext_method ---

#[tokio::test]
async fn ext_method_forwards_request_and_returns_response() {
    let (mock, _clock, bridge) = mock_bridge();
    let raw = RawValue::from_string(r#"{"result":"ok"}"#.to_string()).unwrap();
    let expected = ExtResponse::new(raw.into());
    set_json_response(&mock, "acp.agent.ext.my_method", &expected);

    let params = RawValue::from_string(r#"{"key":"value"}"#.to_string()).unwrap();
    let request = ExtRequest::new("my_method", params.into());
    let result = bridge.ext_method(request).await;

    assert!(result.is_ok());
}

// --- ext_notification ---

#[tokio::test]
async fn ext_notification_publishes_to_nats() {
    let (mock, _clock, bridge) = mock_bridge();

    let params = RawValue::from_string(r#"{"event":"ping"}"#.to_string()).unwrap();
    let notification = ExtNotification::new("my_notify", params.into());
    let result = bridge.ext_notification(notification).await;

    assert!(result.is_ok());

    let published = mock.published_messages();
    assert!(
        published.iter().any(|s| s.contains("agent.ext.my_notify")),
        "Expected ext notification publish, got: {:?}",
        published
    );
}

// --- prompt timeout ---

#[tokio::test(start_paused = true)]
async fn prompt_returns_error_when_response_times_out() {
    let (mock, bridge) = {
        let mock = AdvancedMockNatsClient::new();
        let clock = MockClock::new();
        let config =
            Config::for_test("acp").with_prompt_timeout(std::time::Duration::from_millis(50));
        let bridge = Bridge::new(mock.clone(), clock, &noop_meter(), config);
        (mock, bridge)
    };

    // Note: We set a mock response for the prompt publish, but the actual prompt response
    // comes via the prompt_response notification channel, which times out in this test.
    mock.set_response(
        "acp.timeout-session.agent.session.prompt",
        serde_json::to_vec(&serde_json::json!({})).unwrap().into(),
    );

    let request = PromptRequest::new("timeout-session", vec![ContentBlock::from("hello")]);
    let result = bridge.prompt(request).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.message.contains("timed out"),
        "Expected 'timed out' in error message, got: {}",
        err.message,
    );
}

// --- custom prefix ---

#[tokio::test]
async fn handlers_use_custom_prefix() {
    let mock = AdvancedMockNatsClient::new();
    let clock = MockClock::new();
    let bridge = Bridge::new(
        mock.clone(),
        clock,
        &noop_meter(),
        Config::for_test("myorg.prod"),
    );

    let expected = InitializeResponse::new(ProtocolVersion::LATEST);
    set_json_response(&mock, "myorg.prod.agent.initialize", &expected);

    let request = InitializeRequest::new(ProtocolVersion::LATEST);
    let result = bridge.initialize(request).await;

    assert!(result.is_ok());
}
