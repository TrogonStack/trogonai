//! Unit tests for `create_sub_session` and `run_sub_session`.
//!
//! These tests use the same mock infrastructure that `acp-nats`'s own unit tests
//! use (`AdvancedMockNatsClient` + hand-rolled `MockJs`), avoiding any running
//! NATS server.
//!
//! Run with:
//!   cargo test -p trogon-runner-tools --test spawn_session_tests

use agent_client_protocol::{NewSessionResponse, PromptResponse, SessionId, StopReason};
use acp_nats::{AcpPrefix, Bridge, Config};
use trogon_nats::jetstream::{
    JetStreamGetStream, JetStreamPublisher, MockJetStreamConsumerFactory,
    MockJetStreamConsumer, MockJetStreamPublisher, MockJetStreamStream, MockJsMessage,
};
use trogon_nats::{AdvancedMockNatsClient, NatsAuth, NatsConfig};
use trogon_runner_tools::spawn_session::{create_sub_session, run_sub_session};
use trogon_std::time::MockClock;

// ── Local MockJs (mirrors acp-nats's pub(crate) test_support::MockJs) ─────────

#[derive(Clone)]
struct MockJs {
    publisher: MockJetStreamPublisher,
    consumer_factory: MockJetStreamConsumerFactory,
}

impl MockJs {
    fn new() -> Self {
        Self {
            publisher: MockJetStreamPublisher::new(),
            consumer_factory: MockJetStreamConsumerFactory::new(),
        }
    }
}

impl JetStreamPublisher for MockJs {
    type PublishError = trogon_nats::mocks::MockError;
    type AckFuture = std::future::Ready<Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>>;

    async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
        &self,
        subject: S,
        headers: async_nats::HeaderMap,
        payload: bytes::Bytes,
    ) -> Result<Self::AckFuture, Self::PublishError> {
        self.publisher.publish_with_headers(subject, headers, payload).await
    }
}

impl JetStreamGetStream for MockJs {
    type Error = trogon_nats::mocks::MockError;
    type Stream = MockJetStreamStream;

    async fn get_stream<T: AsRef<str> + Send>(
        &self,
        stream_name: T,
    ) -> Result<MockJetStreamStream, Self::Error> {
        self.consumer_factory.get_stream(stream_name).await
    }
}

// ── Helper to build a mock bridge ─────────────────────────────────────────────

fn make_mock_bridge(
    prefix: &str,
) -> (
    AdvancedMockNatsClient,
    MockJs,
    Bridge<AdvancedMockNatsClient, MockClock, MockJs>,
) {
    let nats_config = NatsConfig::new(vec!["localhost:4222".to_string()], NatsAuth::None);
    let acp_prefix = AcpPrefix::new(prefix.to_string()).expect("valid prefix");
    let config = Config::new(acp_prefix, nats_config);

    let mock = AdvancedMockNatsClient::new();
    let js = MockJs::new();

    let meter = opentelemetry::global::meter("spawn-session-tests");
    let (tx, _rx) = tokio::sync::mpsc::channel(1);

    let bridge = Bridge::new(mock.clone(), js.clone(), MockClock::new(), &meter, config, tx);

    (mock, js, bridge)
}

/// Helpers to enqueue a JS response by serialising to JSON and pushing it through
/// a `MockJetStreamConsumer`.
fn enqueue_js_response<T: serde::Serialize>(js: &MockJs, resp: &T) {
    let bytes = serde_json::to_vec(resp).unwrap();
    let msg = MockJsMessage::new(async_nats::Message {
        subject: "test".into(),
        reply: None,
        payload: bytes::Bytes::from(bytes),
        headers: None,
        status: None,
        description: None,
        length: 0,
    });
    let (consumer, tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
    tx.unbounded_send(Ok(msg)).unwrap();
    js.consumer_factory.add_consumer(consumer);
}

// ── create_sub_session ────────────────────────────────────────────────────────

/// `create_sub_session` returns a non-empty session ID string on success.
///
/// In "default" mode it only sends a `new_session` NATS request-reply and then
/// attempts a best-effort `set_session_mode` (which we don't need to pre-load
/// since the call is `.ok()`-discarded).
#[tokio::test]
async fn create_sub_session_returns_session_id() {
    let (mock, _js, bridge) = make_mock_bridge("acp");

    // Pre-load response for new_session (NATS request-reply).
    let session_id = SessionId::from("sub-sess-001");
    let new_session_resp = NewSessionResponse::new(session_id.clone());
    mock.set_response(
        "acp.agent.session.new",
        serde_json::to_vec(&new_session_resp).unwrap().into(),
    );

    let result = create_sub_session(&bridge, "/tmp", "default", None).await;

    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());

    let returned_id = result.unwrap();
    assert!(!returned_id.is_empty(), "session_id must not be empty");
    assert_eq!(returned_id, session_id.to_string());
}

/// When `new_session` fails (NATS returns an error), `create_sub_session` returns
/// an `Err` containing a human-readable message.
#[tokio::test]
async fn create_sub_session_returns_error_on_new_session_failure() {
    let (mock, _js, bridge) = make_mock_bridge("acp");

    // Cause the next NATS request to fail immediately.
    mock.fail_next_request();

    let result = create_sub_session(&bridge, "/tmp", "default", None).await;
    assert!(result.is_err(), "expected Err when new_session fails");
    assert!(
        result.unwrap_err().contains("new_session failed"),
        "error message should mention new_session"
    );
}

/// In `bypassPermissions` mode, `create_sub_session` skips `set_session_mode`
/// entirely, so it returns `Ok` with just a successful `new_session` response.
#[tokio::test]
async fn create_sub_session_bypass_mode_skips_set_session_mode() {
    let (mock, _js, bridge) = make_mock_bridge("acp");

    // Provide a successful new_session response only — no JS response needed.
    let session_id = SessionId::from("sub-sess-002");
    let new_session_resp = NewSessionResponse::new(session_id.clone());
    mock.set_response(
        "acp.agent.session.new",
        serde_json::to_vec(&new_session_resp).unwrap().into(),
    );

    let result = create_sub_session(&bridge, "/tmp", "bypassPermissions", None).await;
    assert!(result.is_ok(), "expected Ok in bypassPermissions mode, got: {:?}", result.unwrap_err());
    assert_eq!(result.unwrap(), session_id.to_string());
}

// ── run_sub_session ───────────────────────────────────────────────────────────

/// `run_sub_session` returns `Ok(())` when both `prompt` and `close_session`
/// succeed.
///
/// `prompt` (via `handle_js`) opens:
///   1. A JetStream consumer for notifications (first `get_stream`)
///   2. A JetStream consumer for the PromptResponse (second `get_stream`)
///   3. A core-NATS `subscribe` for the cancel signal
///
/// We pre-load all three so the select! loop can fire and return Ok.
/// The notification consumer is left open (sender kept alive) — the response
/// consumer delivers the PromptResponse first, causing the loop to break with Ok.
#[tokio::test]
async fn run_sub_session_returns_ok_on_success() {
    let (mock, js, bridge) = make_mock_bridge("acp");

    // (3) Cancel subscription: kept open, never delivers a message.
    let _cancel_tx = mock.inject_messages();

    // (1) Notification consumer: open but empty — sender kept alive so the
    // stream does not close (which would cause "notification stream closed").
    let (notif_consumer, _notif_tx) = MockJetStreamConsumer::new();
    js.consumer_factory.add_consumer(notif_consumer);

    // (2) Response consumer: delivers PromptResponse immediately.
    enqueue_js_response(&js, &PromptResponse::new(StopReason::EndTurn));

    // close_session also uses JetStream (one more get_stream call).
    enqueue_js_response(&js, &agent_client_protocol::CloseSessionResponse::new());

    let result = run_sub_session(&bridge, "sess-run-001", "hello from test", std::time::Duration::from_secs(30)).await;
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
}

/// When `prompt` fails, `run_sub_session` returns `Err`, but still attempts to
/// close the session (best-effort; we verify the error propagates).
///
/// Implementation detail: `close_session` is called with `let _ = ...`, so its
/// result is discarded.  We only assert that the function returns `Err` and that
/// the message mentions "prompt failed".
#[tokio::test]
async fn run_sub_session_returns_error_on_prompt_failure() {
    let (_mock, _js, bridge) = make_mock_bridge("acp");

    // No JetStream responses — prompt will fail immediately.
    let result = run_sub_session(&bridge, "sess-run-002", "fail prompt", std::time::Duration::from_secs(30)).await;

    assert!(result.is_err(), "expected Err when prompt fails");
    assert!(
        result.unwrap_err().contains("prompt failed"),
        "error should mention 'prompt failed'"
    );
}

/// Even when `prompt` fails, the session-close attempt must not panic.
///
/// This test verifies that the `run_sub_session` function does not panic when
/// both `prompt` and `close_session` fail — the close is best-effort.
#[tokio::test]
async fn run_sub_session_does_not_panic_when_both_prompt_and_close_fail() {
    let (_mock, _js, bridge) = make_mock_bridge("acp");

    // No JS responses queued — both prompt and close will fail silently.
    let result = run_sub_session(&bridge, "sess-run-003", "anything", std::time::Duration::from_secs(30)).await;

    // We only care that no panic occurred; the result is an Err.
    assert!(result.is_err());
}
