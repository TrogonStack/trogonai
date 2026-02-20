use crate::agent::Bridge;
use crate::tests::AdvancedMockNatsClient;
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification, ContentBlock,
    ExtNotification, ExtRequest, ExtResponse, InitializeRequest, InitializeResponse,
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest,
    ProtocolVersion, SetSessionModeRequest, SetSessionModeResponse, StopReason,
};
use serde_json::value::RawValue;

fn mock_bridge() -> (AdvancedMockNatsClient, Bridge<AdvancedMockNatsClient>) {
    let mock = AdvancedMockNatsClient::new();
    let bridge = Bridge::new(Some(mock.clone()), "acp".to_string());
    (mock, bridge)
}

fn set_json_response<T: serde::Serialize>(mock: &AdvancedMockNatsClient, subject: &str, resp: &T) {
    let bytes = serde_json::to_vec(resp).unwrap();
    mock.set_response(subject, bytes.into());
}

// --- initialize ---

#[tokio::test]
async fn initialize_forwards_request_and_returns_response() {
    let (mock, bridge) = mock_bridge();
    let expected = InitializeResponse::new(ProtocolVersion::LATEST);
    set_json_response(&mock, "acp.agent.initialize", &expected);

    let request = InitializeRequest::new(ProtocolVersion::LATEST);
    let result = bridge.initialize(request).await;

    assert!(result.is_ok());
    let response = result.unwrap();
    assert_eq!(response.protocol_version, ProtocolVersion::LATEST);
}

#[tokio::test]
async fn initialize_returns_error_when_nats_unavailable() {
    let bridge = Bridge::<AdvancedMockNatsClient>::new(None, "acp".to_string());
    let request = InitializeRequest::new(ProtocolVersion::LATEST);
    let result = bridge.initialize(request).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("NATS connection"));
}

#[tokio::test]
async fn initialize_returns_error_when_nats_request_fails() {
    let (mock, bridge) = mock_bridge();
    mock.fail_next_request();

    let request = InitializeRequest::new(ProtocolVersion::LATEST);
    let result = bridge.initialize(request).await;

    assert!(result.is_err());
}

// --- authenticate ---

#[tokio::test]
async fn authenticate_forwards_request_and_returns_response() {
    let (mock, bridge) = mock_bridge();
    let expected = AuthenticateResponse::new();
    set_json_response(&mock, "acp.agent.authenticate", &expected);

    let request = AuthenticateRequest::new("api-key");
    let result = bridge.authenticate(request).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn authenticate_returns_error_when_nats_unavailable() {
    let bridge = Bridge::<AdvancedMockNatsClient>::new(None, "acp".to_string());
    let request = AuthenticateRequest::new("api-key");
    let result = bridge.authenticate(request).await;

    assert!(result.is_err());
}

// --- new_session ---

#[tokio::test]
async fn new_session_forwards_request_and_returns_response() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (mock, bridge) = mock_bridge();
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
            let (mock, bridge) = mock_bridge();
            let expected = LoadSessionResponse::new();
            set_json_response(
                &mock,
                "acp.session-load-001.agent.session.load",
                &expected,
            );

            let request = LoadSessionRequest::new("session-load-001", "/tmp");
            let result = bridge.load_session(request).await;

            assert!(result.is_ok());
        })
        .await;
}

// --- set_session_mode ---

#[tokio::test]
async fn set_session_mode_forwards_request_and_returns_response() {
    let (mock, bridge) = mock_bridge();
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
    let (_mock, bridge) = mock_bridge();
    let session_id = "cancel-session-001";

    assert!(!bridge.cancelled_sessions.is_cancelled(&session_id.into()));

    let notification = CancelNotification::new(session_id);
    bridge.cancel(notification).await.unwrap();

    assert!(bridge.cancelled_sessions.is_cancelled(&session_id.into()));
}

#[tokio::test]
async fn cancel_resolves_pending_prompt_waiter_with_cancelled() {
    let (_mock, bridge) = mock_bridge();
    let session_id: agent_client_protocol::SessionId = "cancel-session-002".into();

    let rx = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone());

    let notification = CancelNotification::new(session_id.clone());
    bridge.cancel(notification).await.unwrap();

    let response = rx.await.expect("Should receive cancelled response");
    assert_eq!(response.stop_reason, StopReason::Cancelled);
}

#[tokio::test]
async fn cancel_publishes_to_nats() {
    let (mock, bridge) = mock_bridge();
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

// --- prompt ---

#[tokio::test]
async fn prompt_returns_cancelled_when_session_already_cancelled() {
    let (_mock, bridge) = mock_bridge();
    let session_id = "prompt-cancel-001";

    bridge
        .cancelled_sessions
        .mark_cancelled(session_id.into());

    let request = PromptRequest::new(session_id, vec![ContentBlock::from("hello")]);
    let result = bridge.prompt(request).await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().stop_reason, StopReason::Cancelled);

    // Cancellation should be cleared after consuming it
    assert!(!bridge.cancelled_sessions.is_cancelled(&session_id.into()));
}

#[tokio::test]
async fn prompt_returns_error_when_nats_unavailable() {
    let bridge = Bridge::<AdvancedMockNatsClient>::new(None, "acp".to_string());
    let request = PromptRequest::new("session-no-nats", vec![ContentBlock::from("hello")]);
    let result = bridge.prompt(request).await;

    assert!(result.is_err());
}

// --- ext_method ---

#[tokio::test]
async fn ext_method_forwards_request_and_returns_response() {
    let (mock, bridge) = mock_bridge();
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
    let (mock, bridge) = mock_bridge();

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

#[tokio::test]
async fn ext_notification_succeeds_without_nats() {
    let bridge = Bridge::<AdvancedMockNatsClient>::new(None, "acp".to_string());

    let params = RawValue::from_string(r#"{}"#.to_string()).unwrap();
    let notification = ExtNotification::new("my_notify", params.into());
    let result = bridge.ext_notification(notification).await;

    assert!(result.is_ok());
}

// --- custom prefix ---

#[tokio::test]
async fn handlers_use_custom_prefix() {
    let mock = AdvancedMockNatsClient::new();
    let bridge = Bridge::new(Some(mock.clone()), "myorg.prod".to_string());

    let expected = InitializeResponse::new(ProtocolVersion::LATEST);
    set_json_response(&mock, "myorg.prod.agent.initialize", &expected);

    let request = InitializeRequest::new(ProtocolVersion::LATEST);
    let result = bridge.initialize(request).await;

    assert!(result.is_ok());
}
