//! End-to-end integration tests for `trogon-xai-runner`.
//!
//! These tests wire `XaiAgent` through `AgentSideNatsConnection` using
//! `MockNatsClient` as the NATS transport and inline mock implementations of
//! `XaiHttpClient` and `SessionNotifier`.  No real network, no real NATS.
//!
//! Each test body runs inside `LocalSet::run_until(...)` because
//! `AgentSideNatsConnection` spawns local tasks internally.
//!
//! The inline mocks use `Clone` + shared `Arc<Mutex<_>>` interior state so
//! the harness can keep a handle to push responses and read notifications
//! after the agent has taken ownership of the original value.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats_agent::AgentSideNatsConnection;
use agent_client_protocol::{
    CloseSessionRequest, CloseSessionResponse, ContentBlock, InitializeRequest, InitializeResponse,
    NewSessionRequest, PromptRequest, PromptResponse, ProtocolVersion, SessionNotification,
};
use async_nats::Message;
use async_trait::async_trait;
use futures::channel::mpsc::UnboundedSender;
use futures_util::stream::{self, LocalBoxStream};
use futures_util::StreamExt as _;
use trogon_nats::mocks::MockNatsClient;
use trogon_xai_runner::{InputItem, SessionNotifier, XaiAgent, XaiEvent, XaiHttpClient};

// ── Inline mock: xAI HTTP client ─────────────────────────────────────────────
//
// Implements `XaiHttpClient` for the local type directly (required by orphan
// rules — we cannot implement a foreign trait for `Arc<LocalType>`).
// The inner `Arc<Mutex<_>>` state is cheap to clone, so `Harness` keeps its
// own clone to push responses after the agent takes ownership.

#[derive(Clone)]
struct TestHttpClient {
    queue: Arc<Mutex<VecDeque<Vec<XaiEvent>>>>,
}

impl TestHttpClient {
    fn new() -> Self {
        Self { queue: Arc::new(Mutex::new(VecDeque::new())) }
    }

    fn push(&self, events: Vec<XaiEvent>) {
        self.queue.lock().unwrap().push_back(events);
    }
}

#[async_trait(?Send)]
impl XaiHttpClient for TestHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _input: &[InputItem],
        _api_key: &str,
        _tools: &[String],
        _previous_response_id: Option<&str>,
        _max_turns: Option<u32>,
    ) -> LocalBoxStream<'static, XaiEvent> {
        let events = self.queue.lock().unwrap().pop_front().unwrap_or_default();
        stream::iter(events).boxed_local()
    }
}

// ── Inline mock: session notifier ─────────────────────────────────────────────

#[derive(Clone)]
struct TestNotifier {
    notifications: Arc<Mutex<Vec<SessionNotification>>>,
}

impl TestNotifier {
    fn new() -> Self {
        Self { notifications: Arc::new(Mutex::new(Vec::new())) }
    }

    fn count(&self) -> usize {
        self.notifications.lock().unwrap().len()
    }
}

#[async_trait(?Send)]
impl SessionNotifier for TestNotifier {
    async fn notify(&self, notification: SessionNotification) {
        self.notifications.lock().unwrap().push(notification);
    }
}

// ── Test harness ──────────────────────────────────────────────────────────────

struct Harness {
    nats: MockNatsClient,
    /// Cloned handle used to push HTTP responses from test bodies.
    http: TestHttpClient,
    /// Cloned handle used to read notifications from test bodies.
    notifier: TestNotifier,
    /// Sender for global subjects (`acp.agent.*`).
    global_tx: UnboundedSender<Message>,
    /// Sender for session subjects (`acp.session.*.agent.*`).
    session_tx: UnboundedSender<Message>,
}

impl Harness {
    /// Build the harness and start the agent connection task.
    ///
    /// Must be called inside a `LocalSet` (spawns local tasks).
    fn new() -> Self {
        let nats = MockNatsClient::new();
        let http = TestHttpClient::new();
        let notifier = TestNotifier::new();

        // `AgentSideNatsConnection` calls `nats.subscribe()` twice when first polled:
        //   1st call → global subscription stream  (`acp.agent.>`)
        //   2nd call → session subscription stream (`acp.session.*.agent.>`)
        // Pre-register both senders so the subscribe calls succeed immediately.
        let global_tx = nats.inject_messages();
        let session_tx = nats.inject_messages();

        // Clone the handles before the agent takes ownership.
        let http_clone = http.clone();
        let notifier_clone = notifier.clone();

        let agent = XaiAgent::with_deps(notifier_clone, "grok-3", "test-key", http_clone);
        let prefix = AcpPrefix::new("acp").unwrap();
        let (_, io_task) = AgentSideNatsConnection::new(agent, nats.clone(), prefix, |fut| {
            tokio::task::spawn_local(fut);
        });
        tokio::task::spawn_local(async move {
            let _ = io_task.await;
        });

        Self { nats, http, notifier, global_tx, session_tx }
    }

    /// Inject a request on the global stream (e.g. `initialize`, `session.new`).
    fn global(&self, subject: &str, payload: impl serde::Serialize, reply: &str) {
        self.inject(&self.global_tx, subject, payload, Some(reply));
    }

    /// Inject a request on the session stream (e.g. `prompt`, `close`).
    fn session_req(&self, subject: &str, payload: impl serde::Serialize, reply: &str) {
        self.inject(&self.session_tx, subject, payload, Some(reply));
    }

    fn inject(
        &self,
        tx: &UnboundedSender<Message>,
        subject: &str,
        payload: impl serde::Serialize,
        reply: Option<&str>,
    ) {
        let raw: bytes::Bytes = serde_json::to_vec(&payload).unwrap().into();
        let msg = Message {
            subject: subject.into(),
            reply: reply.map(Into::into),
            payload: raw,
            headers: None,
            length: 0,
            status: None,
            description: None,
        };
        tx.unbounded_send(msg).unwrap();
    }

    /// Spin-yield until at least `n` messages have been published, then return them.
    async fn expect_n_publishes(&self, n: usize) -> Vec<bytes::Bytes> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let p = self.nats.published_payloads();
            if p.len() >= n {
                return p;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timeout: expected {} published messages, got {}",
                n,
                p.len()
            );
            tokio::task::yield_now().await;
        }
    }

    /// Spin-yield until the notifier has at least `n` notifications.
    async fn expect_n_notifications(&self, n: usize) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if self.notifier.count() >= n {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timeout: expected {} notifications, got {}",
                n,
                self.notifier.count()
            );
            tokio::task::yield_now().await;
        }
    }
}

// ── Shared helper ─────────────────────────────────────────────────────────────

/// Send `new_session` and return the created session ID.
async fn create_session(h: &Harness) -> String {
    h.global("acp.agent.session.new", NewSessionRequest::new("/tmp"), "r.new");
    let payloads = h.expect_n_publishes(1).await;
    let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
    val["sessionId"].as_str().unwrap().to_string()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// `initialize` dispatches through NATS and returns valid protocol capabilities.
#[tokio::test]
async fn initialize_via_nats_returns_capabilities() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.global(
                "acp.agent.initialize",
                InitializeRequest::new(ProtocolVersion::LATEST),
                "reply.init",
            );
            let payloads = h.expect_n_publishes(1).await;
            let resp: InitializeResponse = serde_json::from_slice(&payloads[0]).unwrap();
            assert_eq!(resp.protocol_version, ProtocolVersion::LATEST);
        })
        .await;
}

/// `new_session` dispatches through NATS and returns a non-empty session ID.
#[tokio::test]
async fn new_session_via_nats_creates_session() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.global(
                "acp.agent.session.new",
                NewSessionRequest::new("/workspace"),
                "reply.new",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            let sid = val["sessionId"].as_str().unwrap_or_default();
            assert!(!sid.is_empty(), "session_id must not be empty");
        })
        .await;
}

/// Full round-trip: `new_session` → `prompt` → valid `PromptResponse` published.
#[tokio::test]
async fn prompt_via_nats_returns_prompt_response() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::TextDelta { text: "hello".to_string() },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{}.agent.prompt", sid);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("ping")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(2).await;
            // Second published payload is the PromptResponse.
            let _: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
        })
        .await;
}

/// Each `TextDelta` event produces a `SessionNotification` via the notifier.
#[tokio::test]
async fn prompt_via_nats_sends_session_notifications() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::TextDelta { text: "chunk1".to_string() },
                XaiEvent::TextDelta { text: "chunk2".to_string() },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{}.agent.prompt", sid);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );
            // Wait for prompt to complete, then verify notification count.
            h.expect_n_publishes(2).await;
            h.expect_n_notifications(2).await;
            assert_eq!(h.notifier.count(), 2);
        })
        .await;
}

/// `close_session` dispatches through NATS and returns a valid response.
#[tokio::test]
async fn close_session_via_nats_returns_ok() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let close_subj = format!("acp.session.{}.agent.close", sid);
            h.session_req(&close_subj, CloseSessionRequest::new(sid.clone()), "r.close");

            let payloads = h.expect_n_publishes(2).await;
            let _: CloseSessionResponse = serde_json::from_slice(&payloads[1]).unwrap();
        })
        .await;
}

/// Prompting a non-existent session returns an ACP protocol error (`code` field present).
#[tokio::test]
async fn prompt_unknown_session_returns_acp_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();

            h.session_req(
                "acp.session.no-such-session.agent.prompt",
                PromptRequest::new("no-such-session", vec![ContentBlock::from("hi")]),
                "r.err",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "expected ACP error with 'code' field, got: {}",
                val
            );
        })
        .await;
}
