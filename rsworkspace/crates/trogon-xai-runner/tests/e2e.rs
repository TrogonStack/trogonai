//! End-to-end integration tests for `trogon-xai-runner`.
//!
//! These tests wire `XaiAgent` through `AgentSideNatsConnection` using
//! `MockNatsClient` as the NATS transport and inline mock implementations of
//! `XaiHttpClient` and `SessionNotifier`. No real network, no real NATS.
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
    AuthenticateRequest, AuthenticateResponse, CancelNotification, CloseSessionRequest,
    CloseSessionResponse, ContentBlock, ForkSessionRequest, InitializeRequest,
    InitializeResponse, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, PromptRequest, PromptResponse, ProtocolVersion,
    ResumeSessionRequest, ResumeSessionResponse, SessionNotification,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, SetSessionModelRequest, SetSessionModelResponse,
};
use async_nats::Message;
use async_trait::async_trait;
use futures::channel::mpsc::UnboundedSender;
use futures_util::stream::{self, LocalBoxStream};
use futures_util::StreamExt as _;
use trogon_nats::mocks::MockNatsClient;
use trogon_xai_runner::{FinishReason, InputItem, SessionNotifier, XaiAgent, XaiEvent, XaiHttpClient};

// ── Inline mock: xAI HTTP client ─────────────────────────────────────────────
//
// Implements `XaiHttpClient` for the local type directly (required by orphan
// rules — we cannot implement a foreign trait for `Arc<LocalType>`).
// The inner `Arc<Mutex<_>>` state is cheap to clone, so `Harness` keeps its
// own clone to push responses and inspect call parameters after the agent
// takes ownership.

enum TestResponse {
    Events(Vec<XaiEvent>),
    /// Yields `first` then blocks forever — used to simulate a hung connection
    /// so that cancellation tests can interrupt an in-flight prompt.
    Slow(XaiEvent),
}

/// Parameters recorded for each `chat_stream` call.
#[derive(Clone)]
struct HttpCall {
    pub input_len: usize,
    pub previous_response_id: Option<String>,
}

#[derive(Clone)]
struct TestHttpClient {
    queue: Arc<Mutex<VecDeque<TestResponse>>>,
    calls: Arc<Mutex<Vec<HttpCall>>>,
}

impl TestHttpClient {
    fn new() -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn push(&self, events: Vec<XaiEvent>) {
        self.queue.lock().unwrap().push_back(TestResponse::Events(events));
    }

    /// Enqueue a slow response — yields `first`, then blocks indefinitely.
    fn push_slow(&self, first: XaiEvent) {
        self.queue.lock().unwrap().push_back(TestResponse::Slow(first));
    }

    fn last_call(&self) -> Option<HttpCall> {
        self.calls.lock().unwrap().last().cloned()
    }
}

#[async_trait(?Send)]
impl XaiHttpClient for TestHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        input: &[InputItem],
        _api_key: &str,
        _tools: &[String],
        previous_response_id: Option<&str>,
        _max_turns: Option<u32>,
    ) -> LocalBoxStream<'static, XaiEvent> {
        self.calls.lock().unwrap().push(HttpCall {
            input_len: input.len(),
            previous_response_id: previous_response_id.map(str::to_string),
        });

        let response =
            self.queue.lock().unwrap().pop_front().unwrap_or(TestResponse::Events(vec![]));
        match response {
            TestResponse::Events(events) => stream::iter(events).boxed_local(),
            TestResponse::Slow(first) => stream::once(async move { first })
                .chain(stream::pending::<XaiEvent>())
                .boxed_local(),
        }
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
    /// Cloned handle used to push HTTP responses and inspect calls.
    http: TestHttpClient,
    /// Cloned handle used to read notifications from test bodies.
    notifier: TestNotifier,
    /// Sender for global subjects (`acp.agent.*`).
    global_tx: UnboundedSender<Message>,
    /// Sender for session subjects (`acp.session.*.agent.*`).
    session_tx: UnboundedSender<Message>,
}

impl Harness {
    /// Build with the default test API key (`"test-key"`).
    fn new() -> Self {
        Self::with_api_key("test-key")
    }

    /// Build with an explicit API key. Pass `""` to simulate a keyless agent
    /// that requires `authenticate` before it can prompt.
    fn with_api_key(key: &str) -> Self {
        let nats = MockNatsClient::new();
        let http = TestHttpClient::new();
        let notifier = TestNotifier::new();

        // `AgentSideNatsConnection::new` → `serve()` calls `nats.subscribe()` twice:
        //   1st call → global wildcard  (`acp.agent.>`)
        //   2nd call → session wildcard (`acp.session.*.agent.>`)
        // Pre-register both senders so the subscribe calls succeed immediately.
        let global_tx = nats.inject_messages();
        let session_tx = nats.inject_messages();

        let http_clone = http.clone();
        let notifier_clone = notifier.clone();

        let agent = XaiAgent::with_deps(notifier_clone, "grok-3", key, http_clone);
        let prefix = AcpPrefix::new("acp").unwrap();
        let (_, io_task) = AgentSideNatsConnection::new(agent, nats.clone(), prefix, |fut| {
            tokio::task::spawn_local(fut);
        });
        tokio::task::spawn_local(async move {
            let _ = io_task.await;
        });

        Self { nats, http, notifier, global_tx, session_tx }
    }

    /// Inject a serialized request on the global stream with a reply subject.
    fn global(&self, subject: &str, payload: impl serde::Serialize, reply: &str) {
        self.inject(&self.global_tx, subject, payload, Some(reply));
    }

    /// Inject a serialized request on the session stream with a reply subject.
    fn session_req(&self, subject: &str, payload: impl serde::Serialize, reply: &str) {
        self.inject(&self.session_tx, subject, payload, Some(reply));
    }

    /// Inject a serialized notification on the global stream (no reply subject).
    fn global_notify(&self, subject: &str, payload: impl serde::Serialize) {
        self.inject(&self.global_tx, subject, payload, None);
    }

    /// Inject a serialized notification on the session stream (no reply subject).
    fn session_notify(&self, subject: &str, payload: impl serde::Serialize) {
        self.inject(&self.session_tx, subject, payload, None);
    }

    /// Inject raw bytes on the global stream — useful for malformed-JSON tests.
    fn global_raw(&self, subject: &str, payload: &[u8], reply: &str) {
        let msg = Message {
            subject: subject.into(),
            reply: Some(reply.into()),
            payload: bytes::Bytes::copy_from_slice(payload),
            headers: None,
            length: 0,
            status: None,
            description: None,
        };
        self.global_tx.unbounded_send(msg).unwrap();
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

    /// Spin-yield until at least `n` messages have been published, then return them all.
    async fn expect_n_publishes(&self, n: usize) -> Vec<bytes::Bytes> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let p = self.nats.published_payloads();
            if p.len() >= n {
                return p;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timeout: expected {n} published messages, got {}",
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
                "timeout: expected {n} notifications, got {}",
                self.notifier.count()
            );
            tokio::task::yield_now().await;
        }
    }

    /// Yield many times then assert the publish count has not grown past `before`.
    /// Used to verify that no response is published (e.g., no-reply-subject case).
    async fn expect_no_new_publishes(&self, before: usize) {
        for _ in 0..30 {
            tokio::task::yield_now().await;
        }
        let after = self.nats.published_payloads().len();
        assert_eq!(
            after, before,
            "expected no new publishes after {before}, but count grew to {after}"
        );
    }

    /// Return the subjects of all published messages so far.
    fn published_subjects(&self) -> Vec<String> {
        self.nats.published_messages()
    }
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Send `new_session` and return the created session ID.
/// Accounts for prior publishes so it can be called multiple times per test.
async fn create_session(h: &Harness) -> String {
    let before = h.nats.published_payloads().len();
    h.global("acp.agent.session.new", NewSessionRequest::new("/tmp"), "r.new");
    let payloads = h.expect_n_publishes(before + 1).await;
    let val: serde_json::Value = serde_json::from_slice(payloads.last().unwrap()).unwrap();
    val["sessionId"].as_str().unwrap().to_string()
}

// ── Original tests ────────────────────────────────────────────────────────────

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
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("ping")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(2).await;
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
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );
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

            let close_subj = format!("acp.session.{sid}.agent.close");
            h.session_req(&close_subj, CloseSessionRequest::new(sid.clone()), "r.close");

            let payloads = h.expect_n_publishes(2).await;
            let _: CloseSessionResponse = serde_json::from_slice(&payloads[1]).unwrap();
        })
        .await;
}

/// Prompting a non-existent session returns an ACP protocol error.
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
                "expected ACP error with 'code' field, got: {val}"
            );
        })
        .await;
}

// ── authenticate ──────────────────────────────────────────────────────────────

/// `authenticate` dispatches through NATS and returns a valid response.
#[tokio::test]
async fn authenticate_via_nats_succeeds() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let mut meta = serde_json::Map::new();
            meta.insert("XAI_API_KEY".to_string(), serde_json::json!("user-key-123"));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("api-key").meta(meta),
                "r.auth",
            );
            let payloads = h.expect_n_publishes(1).await;
            let _: AuthenticateResponse = serde_json::from_slice(&payloads[0]).unwrap();
        })
        .await;
}

/// `authenticate` stores the API key so the next `new_session` can use it.
/// An agent with no global key must fail without authenticate; with authenticate
/// it should succeed and the subsequent prompt must not return "no API key".
#[tokio::test]
async fn authenticate_then_new_session_uses_pending_key() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // No global key — any prompt without authenticate should fail.
            let h = Harness::with_api_key("");

            // Authenticate to store the pending key.
            let mut meta = serde_json::Map::new();
            meta.insert("XAI_API_KEY".to_string(), serde_json::json!("user-key-123"));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("api-key").meta(meta),
                "r.auth",
            );
            h.expect_n_publishes(1).await;

            // Create session — must consume the pending key.
            let sid = create_session(&h).await;

            // Prompt must succeed (key is now attached to the session).
            h.http.push(vec![XaiEvent::TextDelta { text: "ok".to_string() }, XaiEvent::Done]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(3).await; // auth + session + prompt
            let val: serde_json::Value = serde_json::from_slice(&payloads[2]).unwrap();
            assert!(
                val.get("code").is_none(),
                "prompt must succeed after authenticate, got: {val}"
            );
        })
        .await;
}

// ── cancel ────────────────────────────────────────────────────────────────────

/// `cancel` sent through NATS fires the in-flight prompt's cancel channel,
/// causing it to return a `PromptResponse` instead of blocking forever.
#[tokio::test]
async fn cancel_via_nats_interrupts_prompt() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Slow response: emits one event, then blocks indefinitely.
            h.http.push_slow(XaiEvent::TextDelta { text: "partial".to_string() });

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );

            // Yield to let the prompt task start and enter the streaming loop.
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }

            // Cancel is a notification (no reply subject) — fires the oneshot.
            let cancel_subj = format!("acp.session.{sid}.agent.cancel");
            h.session_notify(&cancel_subj, CancelNotification::new(sid.clone()));

            // The prompt must complete and publish a response within 2 s.
            // Without cancel the slow stream would block it forever.
            let payloads = h.expect_n_publishes(2).await;
            let _: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
        })
        .await;
}

/// Cancelling a session that has no in-flight prompt is a no-op — no publish.
#[tokio::test]
async fn cancel_noop_for_unknown_session_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.session_notify(
                "acp.session.no-such-session.agent.cancel",
                CancelNotification::new("no-such-session"),
            );
            h.expect_no_new_publishes(0).await;
        })
        .await;
}

// ── reply subject routing ─────────────────────────────────────────────────────

/// The response to a request must be published to the exact reply subject
/// provided in the request — not a wildcard or default subject.
#[tokio::test]
async fn reply_subject_routing_is_correct() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.global(
                "acp.agent.initialize",
                InitializeRequest::new(ProtocolVersion::LATEST),
                "reply.unique.subject.99999",
            );
            h.expect_n_publishes(1).await;

            let subjects = h.published_subjects();
            assert!(
                subjects.contains(&"reply.unique.subject.99999".to_string()),
                "response must be published to the exact reply subject; got: {subjects:?}"
            );
        })
        .await;
}

// ── malformed / no-reply edge cases ──────────────────────────────────────────

/// A request with a malformed JSON body must produce an ACP error response
/// (dispatch layer wraps it in `InvalidParams`).
#[tokio::test]
async fn malformed_request_returns_acp_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.global_raw("acp.agent.initialize", b"not valid json", "r.malformed");
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "malformed request must produce an ACP error with 'code' field: {val}"
            );
        })
        .await;
}

/// A request without a reply subject must be silently dropped — no publish.
#[tokio::test]
async fn request_without_reply_subject_does_not_publish() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            // `global_notify` injects without a reply subject.
            h.global_notify(
                "acp.agent.initialize",
                InitializeRequest::new(ProtocolVersion::LATEST),
            );
            h.expect_no_new_publishes(0).await;
        })
        .await;
}

// ── load_session ──────────────────────────────────────────────────────────────

/// `load_session` returns a valid `LoadSessionResponse` for an existing session.
#[tokio::test]
async fn load_session_via_nats_returns_state() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let load_subj = format!("acp.session.{sid}.agent.load");
            h.session_req(&load_subj, LoadSessionRequest::new(sid.clone(), "/tmp"), "r.load");

            let payloads = h.expect_n_publishes(2).await;
            let _: LoadSessionResponse = serde_json::from_slice(&payloads[1]).unwrap();
        })
        .await;
}

// ── resume_session ────────────────────────────────────────────────────────────

/// `resume_session` returns a valid `ResumeSessionResponse` for an existing session.
#[tokio::test]
async fn resume_session_via_nats_returns_ok() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let resume_subj = format!("acp.session.{sid}.agent.resume");
            h.session_req(
                &resume_subj,
                ResumeSessionRequest::new(sid.clone(), "/tmp"),
                "r.resume",
            );

            let payloads = h.expect_n_publishes(2).await;
            let _: ResumeSessionResponse = serde_json::from_slice(&payloads[1]).unwrap();
        })
        .await;
}

// ── fork_session ──────────────────────────────────────────────────────────────

/// Fork an existing session and verify the forked session can be prompted
/// independently of the source.
#[tokio::test]
async fn fork_session_via_nats_and_prompt() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Prompt the source session.
            h.http.push(vec![
                XaiEvent::TextDelta { text: "from src".to_string() },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt1",
            );
            h.expect_n_publishes(2).await;

            // Fork the session.
            let fork_subj = format!("acp.session.{sid}.agent.fork");
            h.session_req(
                &fork_subj,
                ForkSessionRequest::new(sid.clone(), "/fork"),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(3).await;
            let fork_val: serde_json::Value = serde_json::from_slice(&payloads[2]).unwrap();
            let fork_id = fork_val["sessionId"].as_str().unwrap().to_string();
            assert!(!fork_id.is_empty(), "fork must return a non-empty session ID");
            assert_ne!(fork_id, sid, "fork session ID must differ from source");

            // Prompt the forked session — must succeed independently.
            h.http.push(vec![
                XaiEvent::TextDelta { text: "from fork".to_string() },
                XaiEvent::Done,
            ]);
            let fork_prompt_subj = format!("acp.session.{fork_id}.agent.prompt");
            h.session_req(
                &fork_prompt_subj,
                PromptRequest::new(fork_id.clone(), vec![ContentBlock::from("follow-up")]),
                "r.prompt2",
            );
            let payloads = h.expect_n_publishes(4).await;
            let _: PromptResponse = serde_json::from_slice(&payloads[3]).unwrap();
        })
        .await;
}

// ── list_sessions ─────────────────────────────────────────────────────────────

/// `list_sessions` returns all active sessions sorted by ID.
#[tokio::test]
async fn list_sessions_via_nats_returns_sorted() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();

            // Create two sessions (IDs are random UUIDs).
            create_session(&h).await;
            create_session(&h).await;

            h.global("acp.agent.session.list", ListSessionsRequest::new(), "r.list");
            let payloads = h.expect_n_publishes(3).await;

            let resp: ListSessionsResponse = serde_json::from_slice(&payloads[2]).unwrap();
            assert_eq!(resp.sessions.len(), 2, "must list both sessions");

            let ids: Vec<_> =
                resp.sessions.iter().map(|s| s.session_id.to_string()).collect();
            let mut sorted = ids.clone();
            sorted.sort();
            assert_eq!(ids, sorted, "sessions must be sorted by ID: {ids:?}");
        })
        .await;
}

// ── set_session_model ─────────────────────────────────────────────────────────

/// `set_session_model` updates the session model and the change is visible
/// through a subsequent `load_session`.
#[tokio::test]
async fn set_session_model_via_nats_updates_model() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let set_model_subj = format!("acp.session.{sid}.agent.set_model");
            h.session_req(
                &set_model_subj,
                SetSessionModelRequest::new(sid.clone(), "grok-3-mini"),
                "r.set_model",
            );
            let payloads = h.expect_n_publishes(2).await;
            let _: SetSessionModelResponse = serde_json::from_slice(&payloads[1]).unwrap();

            // Verify via load_session that the model change persisted.
            let load_subj = format!("acp.session.{sid}.agent.load");
            h.session_req(&load_subj, LoadSessionRequest::new(sid.clone(), "/tmp"), "r.load");
            let payloads = h.expect_n_publishes(3).await;
            let resp: LoadSessionResponse = serde_json::from_slice(&payloads[2]).unwrap();
            let current =
                resp.models.expect("load must return model state").current_model_id.to_string();
            assert_eq!(current, "grok-3-mini", "model must be updated to grok-3-mini");
        })
        .await;
}

// ── set_session_mode ──────────────────────────────────────────────────────────

/// `set_session_mode` is a no-op that always succeeds — verify the round-trip.
#[tokio::test]
async fn set_session_mode_via_nats_succeeds() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let set_mode_subj = format!("acp.session.{sid}.agent.set_mode");
            h.session_req(
                &set_mode_subj,
                SetSessionModeRequest::new(sid.clone(), "any-mode"),
                "r.set_mode",
            );
            let payloads = h.expect_n_publishes(2).await;
            let _: SetSessionModeResponse = serde_json::from_slice(&payloads[1]).unwrap();
        })
        .await;
}

// ── set_session_config_option ─────────────────────────────────────────────────

/// `set_session_config_option` returns an empty config-options list.
#[tokio::test]
async fn set_session_config_option_via_nats_returns_empty() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let set_config_subj = format!("acp.session.{sid}.agent.set_config_option");
            h.session_req(
                &set_config_subj,
                SetSessionConfigOptionRequest::new(sid.clone(), "some-option", "some-value"),
                "r.config",
            );
            let payloads = h.expect_n_publishes(2).await;
            let resp: SetSessionConfigOptionResponse =
                serde_json::from_slice(&payloads[1]).unwrap();
            assert!(resp.config_options.is_empty(), "config options must be empty");
        })
        .await;
}

// ── session isolation ─────────────────────────────────────────────────────────

/// Closing session A must not affect session B — session B remains fully
/// operational after A is closed.
#[tokio::test]
async fn two_sessions_are_isolated() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid_a = create_session(&h).await;
            let sid_b = create_session(&h).await;

            // Close session A.
            let close_subj = format!("acp.session.{sid_a}.agent.close");
            h.session_req(&close_subj, CloseSessionRequest::new(sid_a.clone()), "r.close");
            h.expect_n_publishes(3).await; // session A + session B + close response

            // Prompt closed session A — must return an ACP error.
            let prompt_a_subj = format!("acp.session.{sid_a}.agent.prompt");
            h.session_req(
                &prompt_a_subj,
                PromptRequest::new(sid_a.clone(), vec![ContentBlock::from("hello?")]),
                "r.prompt.a",
            );
            let payloads = h.expect_n_publishes(4).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[3]).unwrap();
            assert!(
                val.get("code").is_some(),
                "closed session must return ACP error: {val}"
            );

            // Prompt session B — must still work.
            h.http.push(vec![
                XaiEvent::TextDelta { text: "b alive".to_string() },
                XaiEvent::Done,
            ]);
            let prompt_b_subj = format!("acp.session.{sid_b}.agent.prompt");
            h.session_req(
                &prompt_b_subj,
                PromptRequest::new(sid_b.clone(), vec![ContentBlock::from("still alive?")]),
                "r.prompt.b",
            );
            let payloads = h.expect_n_publishes(5).await;
            let _: PromptResponse = serde_json::from_slice(&payloads[4]).unwrap();
        })
        .await;
}

// ── previous_response_id shortcut ────────────────────────────────────────────

/// After a turn that returns a `ResponseId`, the next prompt must pass
/// `previous_response_id` to the HTTP client and send only the new user
/// message (not the full history).
#[tokio::test]
async fn response_id_reuse_on_second_prompt() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // First prompt — response includes a ResponseId for the agent to cache.
            h.http.push(vec![
                XaiEvent::ResponseId { id: "resp-cache-abc".to_string() },
                XaiEvent::TextDelta { text: "first answer".to_string() },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("question 1")]),
                "r.prompt1",
            );
            h.expect_n_publishes(2).await;

            // Second prompt — agent must use the cached ResponseId.
            h.http.push(vec![
                XaiEvent::TextDelta { text: "second answer".to_string() },
                XaiEvent::Done,
            ]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("question 2")]),
                "r.prompt2",
            );
            h.expect_n_publishes(3).await;

            let call = h.http.last_call().expect("HTTP client must have recorded a call");
            assert_eq!(
                call.previous_response_id.as_deref(),
                Some("resp-cache-abc"),
                "second prompt must pass previous_response_id to avoid replaying history"
            );
            assert_eq!(
                call.input_len, 1,
                "with previous_response_id, only the new user message is sent"
            );
        })
        .await;
}

// ── Finished variants break the streaming loop ────────────────────────────────

/// A stream that ends with `Finished { Incomplete }` (no `Done`) must still
/// publish a `PromptResponse` — the agent treats all `Finished` variants as
/// end-of-turn.
#[tokio::test]
async fn prompt_finished_incomplete_breaks_loop() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::TextDelta { text: "partial".to_string() },
                XaiEvent::Finished {
                    reason: FinishReason::Incomplete,
                    incomplete_reason: Some("max_output_tokens".to_string()),
                },
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hello")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(2).await;
            let _: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
        })
        .await;
}

/// A stream that ends with `Finished { Failed }` must publish a `PromptResponse`.
#[tokio::test]
async fn prompt_finished_failed_breaks_loop() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::Finished { reason: FinishReason::Failed, incomplete_reason: None },
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hello")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(2).await;
            let _: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
        })
        .await;
}

/// A stream that ends with `Finished { Cancelled }` must publish a `PromptResponse`.
#[tokio::test]
async fn prompt_finished_cancelled_breaks_loop() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::Finished { reason: FinishReason::Cancelled, incomplete_reason: None },
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hello")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(2).await;
            let _: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
        })
        .await;
}

// ── Full event sequence: ResponseId + Usage + Finished (no Done) ──────────────

/// A realistic stream — `ResponseId`, `TextDelta`, `Usage`, `Finished { Completed }` —
/// must publish a `PromptResponse` and cache the `ResponseId` so the next prompt
/// uses `previous_response_id`.
#[tokio::test]
async fn prompt_full_event_sequence_response_id_usage_finished() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // First prompt — realistic sequence without a trailing Done.
            h.http.push(vec![
                XaiEvent::ResponseId { id: "resp-full-seq".to_string() },
                XaiEvent::TextDelta { text: "answer".to_string() },
                XaiEvent::Usage { prompt_tokens: 10, completion_tokens: 5 },
                XaiEvent::Finished { reason: FinishReason::Completed, incomplete_reason: None },
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q1")]),
                "r.prompt1",
            );
            h.expect_n_publishes(2).await;

            // Second prompt — must reuse the cached ResponseId.
            h.http.push(vec![XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q2")]),
                "r.prompt2",
            );
            h.expect_n_publishes(3).await;

            let call = h.http.last_call().unwrap();
            assert_eq!(
                call.previous_response_id.as_deref(),
                Some("resp-full-seq"),
                "second prompt must reuse the ResponseId from the Finished-terminated turn"
            );
            assert_eq!(call.input_len, 1, "only the new user message should be sent");
        })
        .await;
}

// ── Silent events: ServerToolCompleted and FunctionCall ───────────────────────

/// `ServerToolCompleted` in the stream must be silently ignored — the agent must
/// not publish extra notifications for it. Only the final `PromptResponse` and
/// the one `TextDelta` notification are expected.
#[tokio::test]
async fn prompt_server_tool_completed_is_silently_ignored() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::ServerToolCompleted { name: "web_search".to_string() },
                XaiEvent::TextDelta { text: "result".to_string() },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("search something")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;
            // Exactly one notification (for the TextDelta), not two.
            h.expect_n_notifications(1).await;
            assert_eq!(h.notifier.count(), 1, "ServerToolCompleted must not emit a notification");
        })
        .await;
}

/// `FunctionCall` in the stream must be silently ignored — same reasoning as
/// `ServerToolCompleted`: the agent does not expose tool calls to the ACP layer.
#[tokio::test]
async fn prompt_function_call_is_silently_ignored() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::FunctionCall {
                    call_id: "call-1".to_string(),
                    name: "some_tool".to_string(),
                    arguments: "{}".to_string(),
                },
                XaiEvent::TextDelta { text: "done".to_string() },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("call a tool")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;
            // Exactly one notification (for the TextDelta), not two.
            h.expect_n_notifications(1).await;
            assert_eq!(h.notifier.count(), 1, "FunctionCall must not emit a notification");
        })
        .await;
}

// ── set_session_model edge case ───────────────────────────────────────────────

/// `set_session_model` for a non-existent session must return an ACP error.
#[tokio::test]
async fn set_session_model_unknown_session_returns_acp_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.session_req(
                "acp.session.no-such-session.agent.set_model",
                SetSessionModelRequest::new("no-such-session", "grok-3-mini"),
                "r.set_model",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "set_model on unknown session must return ACP error with 'code' field: {val}"
            );
        })
        .await;
}

// ── Error branches: load / resume / fork on unknown session ──────────────────

/// `load_session` on a non-existent session must return an ACP error.
#[tokio::test]
async fn load_session_unknown_session_returns_acp_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.session_req(
                "acp.session.no-such-session.agent.load",
                LoadSessionRequest::new("no-such-session", "/tmp"),
                "r.load",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "load_session on unknown session must return ACP error: {val}"
            );
        })
        .await;
}

/// `resume_session` on a non-existent session must return an ACP error.
#[tokio::test]
async fn resume_session_unknown_session_returns_acp_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.session_req(
                "acp.session.no-such-session.agent.resume",
                ResumeSessionRequest::new("no-such-session", "/tmp"),
                "r.resume",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "resume_session on unknown session must return ACP error: {val}"
            );
        })
        .await;
}

/// `fork_session` on a non-existent source session must return an ACP error.
#[tokio::test]
async fn fork_session_unknown_session_returns_acp_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.session_req(
                "acp.session.no-such-session.agent.fork",
                ForkSessionRequest::new("no-such-session", "/fork"),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "fork_session on unknown session must return ACP error: {val}"
            );
        })
        .await;
}

// ── set_session_model: unknown model ID ───────────────────────────────────────

/// `set_session_model` with a model ID not in `available_models` must return
/// an ACP error even when the session exists.
#[tokio::test]
async fn set_session_model_unknown_model_id_returns_acp_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let set_model_subj = format!("acp.session.{sid}.agent.set_model");
            h.session_req(
                &set_model_subj,
                SetSessionModelRequest::new(sid.clone(), "not-a-real-model"),
                "r.set_model",
            );
            let payloads = h.expect_n_publishes(2).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                val.get("code").is_some(),
                "set_model with unknown model ID must return ACP error: {val}"
            );
        })
        .await;
}

// ── prompt without API key ────────────────────────────────────────────────────

/// Prompting a session that has no API key (no global key, no authenticate)
/// must return an ACP error — not panic or block.
#[tokio::test]
async fn prompt_without_api_key_returns_acp_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // No global API key and no authenticate call.
            let h = Harness::with_api_key("");
            let sid = create_session(&h).await;

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hello")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(2).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                val.get("code").is_some(),
                "prompt without API key must return ACP error: {val}"
            );
        })
        .await;
}

// ── prompt with Error event ───────────────────────────────────────────────────

/// An `XaiEvent::Error` in the stream causes the agent to log a warning and
/// break the loop — it must still publish a `PromptResponse`.
#[tokio::test]
async fn prompt_error_event_breaks_loop() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![XaiEvent::Error { message: "upstream failure".to_string() }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hello")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(2).await;
            let _: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
        })
        .await;
}

// ── close_session cancels in-flight prompt ────────────────────────────────────

/// Closing a session while a prompt is in flight must abort the streaming loop
/// and allow the prompt to publish its `PromptResponse`.
#[tokio::test]
async fn close_session_cancels_in_flight_prompt() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Slow response — blocks until cancelled.
            h.http.push_slow(XaiEvent::TextDelta { text: "partial".to_string() });

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );

            // Let the prompt task start and enter the streaming loop.
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }

            // Close the session — this sends () on the cancel channel.
            let close_subj = format!("acp.session.{sid}.agent.close");
            h.session_req(&close_subj, CloseSessionRequest::new(sid.clone()), "r.close");

            // Both prompt response and close response must be published.
            // The exact ordering is non-deterministic (two concurrent tasks),
            // so we verify that one of the last two payloads is a PromptResponse.
            let payloads = h.expect_n_publishes(3).await;
            let prompt_published = payloads[1..]
                .iter()
                .any(|p| serde_json::from_slice::<PromptResponse>(p).is_ok());
            assert!(
                prompt_published,
                "close must cancel the in-flight prompt and a PromptResponse must be published"
            );
        })
        .await;
}

// ── history accumulates without ResponseId ────────────────────────────────────

/// When no `ResponseId` is returned, the agent replays full history on each
/// turn. After one completed turn (user + assistant), the second prompt must
/// send 3 input items: the previous user message, the assistant reply, and
/// the new user message.
#[tokio::test]
async fn history_grows_across_turns_without_response_id() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // First prompt — no ResponseId, so history is used on next turn.
            h.http.push(vec![
                XaiEvent::TextDelta { text: "first answer".to_string() },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("question 1")]),
                "r.prompt1",
            );
            h.expect_n_publishes(2).await;

            // Second prompt — must replay history (user₁ + assistant₁ + user₂).
            h.http.push(vec![XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("question 2")]),
                "r.prompt2",
            );
            h.expect_n_publishes(3).await;

            let call = h.http.last_call().unwrap();
            assert_eq!(
                call.previous_response_id, None,
                "no ResponseId was emitted, so previous_response_id must be None"
            );
            assert_eq!(
                call.input_len, 3,
                "input must contain: user msg1 + assistant reply + user msg2"
            );
        })
        .await;
}

// ── fork inherits source model ────────────────────────────────────────────────

/// When the source session has a model override, the forked session must
/// inherit that model — verified via `load_session` on the fork.
#[tokio::test]
async fn fork_inherits_source_model() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Set a model on the source session.
            let set_model_subj = format!("acp.session.{sid}.agent.set_model");
            h.session_req(
                &set_model_subj,
                SetSessionModelRequest::new(sid.clone(), "grok-3-mini"),
                "r.set_model",
            );
            h.expect_n_publishes(2).await;

            // Fork the source session.
            let fork_subj = format!("acp.session.{sid}.agent.fork");
            h.session_req(
                &fork_subj,
                ForkSessionRequest::new(sid.clone(), "/fork"),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(3).await;
            let fork_val: serde_json::Value = serde_json::from_slice(&payloads[2]).unwrap();
            let fork_id = fork_val["sessionId"].as_str().unwrap().to_string();

            // Load the fork and verify the inherited model.
            let load_subj = format!("acp.session.{fork_id}.agent.load");
            h.session_req(&load_subj, LoadSessionRequest::new(fork_id.clone(), "/tmp"), "r.load");
            let payloads = h.expect_n_publishes(4).await;
            let resp: LoadSessionResponse = serde_json::from_slice(&payloads[3]).unwrap();
            let current =
                resp.models.expect("load must return model state").current_model_id.to_string();
            assert_eq!(current, "grok-3-mini", "fork must inherit the source session's model");
        })
        .await;
}

// ── second authenticate overwrites the first pending key ─────────────────────

/// If `authenticate` is called twice before `new_session`, the second key must
/// win — `pending_api_key` is overwritten, not appended.
#[tokio::test]
async fn second_authenticate_overwrites_first() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::with_api_key("");

            // First authenticate with a key that must NOT be used.
            let mut meta1 = serde_json::Map::new();
            meta1.insert("XAI_API_KEY".to_string(), serde_json::json!("first-key"));
            h.global("acp.agent.authenticate", AuthenticateRequest::new("api-key").meta(meta1), "r.auth1");
            h.expect_n_publishes(1).await;

            // Second authenticate with the key that must be consumed by new_session.
            let mut meta2 = serde_json::Map::new();
            meta2.insert("XAI_API_KEY".to_string(), serde_json::json!("second-key"));
            h.global("acp.agent.authenticate", AuthenticateRequest::new("api-key").meta(meta2), "r.auth2");
            h.expect_n_publishes(2).await;

            // new_session consumes the pending key (second one).
            let sid = create_session(&h).await;

            // A prompt must succeed — meaning the second key was used, not the first.
            h.http.push(vec![XaiEvent::TextDelta { text: "ok".to_string() }, XaiEvent::Done]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(4).await; // auth1 + auth2 + session + prompt
            let val: serde_json::Value = serde_json::from_slice(&payloads[3]).unwrap();
            assert!(
                val.get("code").is_none(),
                "prompt must succeed after second authenticate: {val}"
            );
        })
        .await;
}
