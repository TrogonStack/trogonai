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
    CloseSessionResponse, ContentBlock, ExtRequest, ForkSessionRequest, InitializeRequest,
    InitializeResponse, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    ProtocolVersion, ResumeSessionRequest, ResumeSessionResponse, SessionNotification,
    SessionUpdate, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
    SetSessionModeRequest, SetSessionModeResponse, SetSessionModelRequest, SetSessionModelResponse,
};
use async_nats::Message;
use async_trait::async_trait;
use futures::channel::mpsc::UnboundedSender;
use futures_util::StreamExt as _;
use futures_util::stream::{self, LocalBoxStream};
use trogon_nats::mocks::MockNatsClient;
use trogon_xai_runner::{
    FinishReason, InputItem, SessionNotifier, XaiAgent, XaiEvent, XaiHttpClient,
};

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
    pub model: String,
    pub input_len: usize,
    pub inputs: Vec<InputItem>,
    pub previous_response_id: Option<String>,
    pub max_turns: Option<u32>,
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
        self.queue
            .lock()
            .unwrap()
            .push_back(TestResponse::Events(events));
    }

    /// Enqueue a slow response — yields `first`, then blocks indefinitely.
    fn push_slow(&self, first: XaiEvent) {
        self.queue
            .lock()
            .unwrap()
            .push_back(TestResponse::Slow(first));
    }

    fn last_call(&self) -> Option<HttpCall> {
        self.calls.lock().unwrap().last().cloned()
    }
}

#[async_trait(?Send)]
impl XaiHttpClient for TestHttpClient {
    async fn chat_stream(
        &self,
        model: &str,
        input: &[InputItem],
        _api_key: &str,
        _tools: &[String],
        previous_response_id: Option<&str>,
        max_turns: Option<u32>,
    ) -> LocalBoxStream<'static, XaiEvent> {
        self.calls.lock().unwrap().push(HttpCall {
            model: model.to_string(),
            input_len: input.len(),
            inputs: input.to_vec(),
            previous_response_id: previous_response_id.map(str::to_string),
            max_turns,
        });

        let response = self
            .queue
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(TestResponse::Events(vec![]));
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
        Self {
            notifications: Arc::new(Mutex::new(Vec::new())),
        }
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

        Self {
            nats,
            http,
            notifier,
            global_tx,
            session_tx,
        }
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

/// Serialize access to `std::env::set_var` / `remove_var` across tests in this
/// binary, which run in parallel by default. Hold the returned guard for the
/// duration of both the env mutation AND the `Harness::new()` call that reads it.
static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
fn env_lock() -> &'static std::sync::Mutex<()> {
    ENV_LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// Send `new_session` and return the created session ID.
/// Accounts for prior publishes so it can be called multiple times per test.
async fn create_session(h: &Harness) -> String {
    let before = h.nats.published_payloads().len();
    h.global(
        "acp.agent.session.new",
        NewSessionRequest::new("/tmp"),
        "r.new",
    );
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
                XaiEvent::TextDelta {
                    text: "hello".to_string(),
                },
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
                XaiEvent::TextDelta {
                    text: "chunk1".to_string(),
                },
                XaiEvent::TextDelta {
                    text: "chunk2".to_string(),
                },
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
            h.session_req(
                &close_subj,
                CloseSessionRequest::new(sid.clone()),
                "r.close",
            );

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
                AuthenticateRequest::new("xai-api-key").meta(meta),
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
                AuthenticateRequest::new("xai-api-key").meta(meta),
                "r.auth",
            );
            h.expect_n_publishes(1).await;

            // Create session — must consume the pending key.
            let sid = create_session(&h).await;

            // Prompt must succeed (key is now attached to the session).
            h.http.push(vec![
                XaiEvent::TextDelta {
                    text: "ok".to_string(),
                },
                XaiEvent::Done,
            ]);
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
            h.http.push_slow(XaiEvent::TextDelta {
                text: "partial".to_string(),
            });

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
            h.session_req(
                &load_subj,
                LoadSessionRequest::new(sid.clone(), "/tmp"),
                "r.load",
            );

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
                XaiEvent::TextDelta {
                    text: "from src".to_string(),
                },
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
            assert!(
                !fork_id.is_empty(),
                "fork must return a non-empty session ID"
            );
            assert_ne!(fork_id, sid, "fork session ID must differ from source");

            // Prompt the forked session — must succeed independently.
            h.http.push(vec![
                XaiEvent::TextDelta {
                    text: "from fork".to_string(),
                },
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

            h.global(
                "acp.agent.session.list",
                ListSessionsRequest::new(),
                "r.list",
            );
            let payloads = h.expect_n_publishes(3).await;

            let resp: ListSessionsResponse = serde_json::from_slice(&payloads[2]).unwrap();
            assert_eq!(resp.sessions.len(), 2, "must list both sessions");

            let ids: Vec<_> = resp
                .sessions
                .iter()
                .map(|s| s.session_id.to_string())
                .collect();
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
            h.session_req(
                &load_subj,
                LoadSessionRequest::new(sid.clone(), "/tmp"),
                "r.load",
            );
            let payloads = h.expect_n_publishes(3).await;
            let resp: LoadSessionResponse = serde_json::from_slice(&payloads[2]).unwrap();
            let current = resp
                .models
                .expect("load must return model state")
                .current_model_id
                .to_string();
            assert_eq!(
                current, "grok-3-mini",
                "model must be updated to grok-3-mini"
            );
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
async fn set_session_config_option_via_nats_returns_full_config_options() {
    // Per ACP spec, set_config_option must return the full set of config options
    // with current values, even when the option id is unknown.
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
            assert!(
                !resp.config_options.is_empty(),
                "response must include the full set of config options"
            );
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
            h.session_req(
                &close_subj,
                CloseSessionRequest::new(sid_a.clone()),
                "r.close",
            );
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
                XaiEvent::TextDelta {
                    text: "b alive".to_string(),
                },
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
                XaiEvent::ResponseId {
                    id: "resp-cache-abc".to_string(),
                },
                XaiEvent::TextDelta {
                    text: "first answer".to_string(),
                },
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
                XaiEvent::TextDelta {
                    text: "second answer".to_string(),
                },
                XaiEvent::Done,
            ]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("question 2")]),
                "r.prompt2",
            );
            h.expect_n_publishes(3).await;

            let call = h
                .http
                .last_call()
                .expect("HTTP client must have recorded a call");
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
                XaiEvent::TextDelta {
                    text: "partial".to_string(),
                },
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

/// A stream that ends with `Finished { Failed }` must publish an ACP error response.
#[tokio::test]
async fn prompt_finished_failed_breaks_loop() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![XaiEvent::Finished {
                reason: FinishReason::Failed,
                incomplete_reason: None,
            }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hello")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(2).await;
            // Agent surfaces the failure as an ACP error (not a PromptResponse).
            let val: serde_json::Value = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(val.get("code").is_some(), "expected ACP error with 'code', got: {val}");
        })
        .await;
}

/// A stream that ends with `Finished { Cancelled }` must publish an ACP error response.
#[tokio::test]
async fn prompt_finished_cancelled_breaks_loop() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![XaiEvent::Finished {
                reason: FinishReason::Cancelled,
                incomplete_reason: None,
            }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hello")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(2).await;
            // Agent surfaces the cancellation as an ACP error (not a PromptResponse).
            let val: serde_json::Value = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(val.get("code").is_some(), "expected ACP error with 'code', got: {val}");
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
                XaiEvent::ResponseId {
                    id: "resp-full-seq".to_string(),
                },
                XaiEvent::TextDelta {
                    text: "answer".to_string(),
                },
                XaiEvent::Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                },
                XaiEvent::Finished {
                    reason: FinishReason::Completed,
                    incomplete_reason: None,
                },
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
            assert_eq!(
                call.input_len, 1,
                "only the new user message should be sent"
            );
        })
        .await;
}

// ── Tool call notifications ───────────────────────────────────────────────────

/// `ServerToolCompleted` without a preceding `FunctionCall` is a no-op — the
/// agent has no pending call to resolve, so no notification is emitted for it.
/// Only the `TextDelta` notification is expected.
#[tokio::test]
async fn prompt_server_tool_completed_without_prior_call_is_ignored() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::ServerToolCompleted {
                    name: "web_search".to_string(),
                },
                XaiEvent::TextDelta {
                    text: "result".to_string(),
                },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("search something")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;
            // Only the TextDelta notification — no ToolCall(Completed) without a prior FunctionCall.
            h.expect_n_notifications(1).await;
        })
        .await;
}

/// `FunctionCall` emits a `ToolCall(Pending)` notification; a matching
/// `ServerToolCompleted` emits a `ToolCall(Completed)` notification.
/// Total: 3 notifications — Pending + TextDelta + Completed (in stream order,
/// but Completed arrives before Done which triggers the PromptResponse reply).
#[tokio::test]
async fn prompt_function_call_emits_tool_call_notifications() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::FunctionCall {
                    call_id: "call-1".to_string(),
                    name: "web_search".to_string(),
                    arguments: r#"{"query":"test"}"#.to_string(),
                },
                XaiEvent::ServerToolCompleted {
                    name: "web_search".to_string(),
                },
                XaiEvent::TextDelta {
                    text: "done".to_string(),
                },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(
                    sid.clone(),
                    vec![ContentBlock::from("search for something")],
                ),
                "r.prompt",
            );
            // create_session reply + PromptResponse reply = 2 NATS publishes
            h.expect_n_publishes(2).await;
            // 3 notifications via TestNotifier: ToolCall(Pending), ToolCall(Completed), AgentMessageChunk
            h.expect_n_notifications(3).await;
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

/// An `XaiEvent::Error` in the stream causes the agent to surface an ACP error.
#[tokio::test]
async fn prompt_error_event_breaks_loop() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![XaiEvent::Error {
                message: "upstream failure".to_string(),
            }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hello")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(2).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(val.get("code").is_some(), "expected ACP error with 'code', got: {val}");
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
            h.http.push_slow(XaiEvent::TextDelta {
                text: "partial".to_string(),
            });

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
            h.session_req(
                &close_subj,
                CloseSessionRequest::new(sid.clone()),
                "r.close",
            );

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
                XaiEvent::TextDelta {
                    text: "first answer".to_string(),
                },
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
            h.session_req(
                &load_subj,
                LoadSessionRequest::new(fork_id.clone(), "/tmp"),
                "r.load",
            );
            let payloads = h.expect_n_publishes(4).await;
            let resp: LoadSessionResponse = serde_json::from_slice(&payloads[3]).unwrap();
            let current = resp
                .models
                .expect("load must return model state")
                .current_model_id
                .to_string();
            assert_eq!(
                current, "grok-3-mini",
                "fork must inherit the source session's model"
            );
        })
        .await;
}

// ── second authenticate overwrites the first pending key ─────────────────────

// ── list_sessions: closed session excluded ────────────────────────────────────

/// After closing a session, `list_sessions` must not include it.
#[tokio::test]
async fn list_sessions_excludes_closed_session() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid_a = create_session(&h).await;
            let sid_b = create_session(&h).await;

            // Close session A.
            let close_subj = format!("acp.session.{sid_a}.agent.close");
            h.session_req(
                &close_subj,
                CloseSessionRequest::new(sid_a.clone()),
                "r.close",
            );
            h.expect_n_publishes(3).await; // session_a + session_b + close

            // List — must contain only session B.
            h.global(
                "acp.agent.session.list",
                ListSessionsRequest::new(),
                "r.list",
            );
            let payloads = h.expect_n_publishes(4).await;
            let resp: ListSessionsResponse = serde_json::from_slice(&payloads[3]).unwrap();
            assert_eq!(
                resp.sessions.len(),
                1,
                "closed session must be removed from the list"
            );
            assert_eq!(
                resp.sessions[0].session_id.to_string(),
                sid_b,
                "only session B must remain"
            );
        })
        .await;
}

// ── list_sessions: cwd is returned ───────────────────────────────────────────

/// The `cwd` passed to `new_session` must appear in the `list_sessions` response.
#[tokio::test]
async fn list_sessions_returns_cwd() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();

            h.global(
                "acp.agent.session.new",
                NewSessionRequest::new("/my-special-cwd"),
                "r.new",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            let sid = val["sessionId"].as_str().unwrap().to_string();

            h.global(
                "acp.agent.session.list",
                ListSessionsRequest::new(),
                "r.list",
            );
            let payloads = h.expect_n_publishes(2).await;
            let resp: ListSessionsResponse = serde_json::from_slice(&payloads[1]).unwrap();

            let info = resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == sid)
                .expect("session must be listed");
            assert_eq!(
                info.cwd.to_string_lossy(),
                "/my-special-cwd",
                "list_sessions must return the cwd from new_session"
            );
        })
        .await;
}

// ── new_session response includes available models ────────────────────────────

/// The `new_session` response must include the `models` field with a non-empty
/// list of available models and the correct default model ID.
#[tokio::test]
async fn new_session_response_includes_available_models() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.global(
                "acp.agent.session.new",
                NewSessionRequest::new("/workspace"),
                "r.new",
            );
            let payloads = h.expect_n_publishes(1).await;
            let resp: NewSessionResponse = serde_json::from_slice(&payloads[0]).unwrap();
            let models = resp
                .models
                .expect("new_session must include a models field");
            assert!(
                !models.available_models.is_empty(),
                "available_models must not be empty"
            );
            assert_eq!(
                models.current_model_id.to_string(),
                "grok-3",
                "default model must be grok-3"
            );
        })
        .await;
}

// ── fork cwd is independent from source ──────────────────────────────────────

/// The cwd of a forked session is the one passed to `fork_session`, not the
/// source session's cwd.
#[tokio::test]
async fn fork_cwd_is_independent_from_source() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();

            h.global(
                "acp.agent.session.new",
                NewSessionRequest::new("/source-cwd"),
                "r.new",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            let sid = val["sessionId"].as_str().unwrap().to_string();

            let fork_subj = format!("acp.session.{sid}.agent.fork");
            h.session_req(
                &fork_subj,
                ForkSessionRequest::new(sid.clone(), "/fork-cwd"),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(2).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[1]).unwrap();
            let fork_id = val["sessionId"].as_str().unwrap().to_string();

            h.global(
                "acp.agent.session.list",
                ListSessionsRequest::new(),
                "r.list",
            );
            let payloads = h.expect_n_publishes(3).await;
            let resp: ListSessionsResponse = serde_json::from_slice(&payloads[2]).unwrap();

            let src = resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == sid)
                .unwrap();
            let fork = resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == fork_id)
                .unwrap();
            assert_eq!(src.cwd.to_string_lossy(), "/source-cwd");
            assert_eq!(fork.cwd.to_string_lossy(), "/fork-cwd");
        })
        .await;
}

// ── prompt: empty stream returns PromptResponse ───────────────────────────────

/// A stream that yields no events (closes immediately) must still publish a
/// `PromptResponse` — the `Ok(None)` arm in the select hits EndTurn.
#[tokio::test]
async fn prompt_empty_stream_returns_response() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![]); // empty — stream closes immediately
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

// ── fork: first prompt replays inherited history ──────────────────────────────

/// A fork inherits the source's history but has no `last_response_id`. The
/// first prompt on the fork must replay the full history — user₁ + assistant₁
/// + user₂ = 3 input items.
#[tokio::test]
async fn fork_first_prompt_replays_full_history() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Prompt source — no ResponseId, so history is stored.
            h.http.push(vec![
                XaiEvent::TextDelta {
                    text: "assistant reply".to_string(),
                },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("user question")]),
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
            let val: serde_json::Value = serde_json::from_slice(&payloads[2]).unwrap();
            let fork_id = val["sessionId"].as_str().unwrap().to_string();

            // Prompt the fork — must replay inherited history.
            h.http.push(vec![XaiEvent::Done]);
            let fork_prompt_subj = format!("acp.session.{fork_id}.agent.prompt");
            h.session_req(
                &fork_prompt_subj,
                PromptRequest::new(fork_id.clone(), vec![ContentBlock::from("follow-up")]),
                "r.prompt2",
            );
            h.expect_n_publishes(4).await;

            let call = h.http.last_call().unwrap();
            assert_eq!(
                call.previous_response_id, None,
                "fork has no last_response_id — must replay full history"
            );
            assert_eq!(
                call.input_len, 3,
                "input must be: user msg1 + assistant reply + follow-up = 3 items"
            );
        })
        .await;
}

// ── second authenticate overwrites the first pending key ─────────────────────

// ── close_session: unknown session is a no-op ─────────────────────────────────

/// Closing a session that does not exist must return `Ok` (not an error).
/// `sessions.remove()` on a missing key is a silent no-op in Rust.
#[tokio::test]
async fn close_session_unknown_session_is_noop() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.session_req(
                "acp.session.no-such-session.agent.close",
                CloseSessionRequest::new("no-such-session"),
                "r.close",
            );
            let payloads = h.expect_n_publishes(1).await;
            let _: CloseSessionResponse = serde_json::from_slice(&payloads[0]).unwrap();
        })
        .await;
}

// ── prompt: Error event preserves previous ResponseId ────────────────────────

/// Three-turn chain: turn₁ returns a `ResponseId`, turn₂ returns an `Error`
/// (no new `ResponseId`), turn₃ must still use the cached ID from turn₁.
/// The guard `if new_response_id.is_some()` must leave the old ID intact on
/// a stream error.
#[tokio::test]
async fn prompt_error_event_preserves_previous_response_id() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");

            // Turn 1 — gets a ResponseId.
            h.http.push(vec![
                XaiEvent::ResponseId {
                    id: "cached-id".to_string(),
                },
                XaiEvent::Done,
            ]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q1")]),
                "r.p1",
            );
            h.expect_n_publishes(2).await;

            // Turn 2 — stream error, no new ResponseId emitted.
            h.http.push(vec![XaiEvent::Error {
                message: "upstream failure".to_string(),
            }]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q2")]),
                "r.p2",
            );
            h.expect_n_publishes(3).await;

            // Turn 3 — must still use "cached-id" from turn 1.
            h.http.push(vec![XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q3")]),
                "r.p3",
            );
            h.expect_n_publishes(4).await;

            let call = h.http.last_call().unwrap();
            assert_eq!(
                call.previous_response_id.as_deref(),
                Some("cached-id"),
                "stream error must not clear the cached ResponseId"
            );
            assert_eq!(
                call.input_len, 1,
                "ResponseId still valid — only new message sent"
            );
        })
        .await;
}

// ── prompt: no TextDelta → no assistant entry in history ─────────────────────

/// When a turn produces no `TextDelta` events, `assistant_text` is empty and
/// the `if !assistant_text.is_empty()` guard prevents adding an assistant
/// message to history. The next prompt must send 2 items (user₁ + user₂),
/// not 3.
#[tokio::test]
async fn prompt_no_text_delta_skips_assistant_history() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");

            // Turn 1 — no TextDelta, only Done. No assistant entry added to history.
            h.http.push(vec![XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q1")]),
                "r.p1",
            );
            h.expect_n_publishes(2).await;

            // Turn 2 — must replay history: only user₁ + user₂ = 2 items (no assistant₁).
            h.http.push(vec![XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q2")]),
                "r.p2",
            );
            h.expect_n_publishes(3).await;

            let call = h.http.last_call().unwrap();
            assert_eq!(
                call.previous_response_id, None,
                "no ResponseId — must replay history"
            );
            assert_eq!(
                call.input_len, 2,
                "history must contain only user₁ + user₂ (no empty assistant entry)"
            );
        })
        .await;
}

// ── set_session_mode / set_session_config_option: always succeed ──────────────

/// `set_session_mode` ignores the session ID entirely — it must return `Ok`
/// even for a non-existent session (intentional "always accept" contract).
#[tokio::test]
async fn set_session_mode_unknown_session_succeeds() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.session_req(
                "acp.session.no-such-session.agent.set_mode",
                SetSessionModeRequest::new("no-such-session", "any-mode"),
                "r.set_mode",
            );
            let payloads = h.expect_n_publishes(1).await;
            let _: SetSessionModeResponse = serde_json::from_slice(&payloads[0]).unwrap();
        })
        .await;
}

/// `set_session_config_option` on an unknown session returns an ACP error.
#[tokio::test]
async fn set_session_config_option_unknown_session_succeeds() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.session_req(
                "acp.session.no-such-session.agent.set_config_option",
                SetSessionConfigOptionRequest::new("no-such-session", "opt", "val"),
                "r.cfg",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(val.get("code").is_some(), "expected ACP error with 'code', got: {val}");
        })
        .await;
}

// ── authenticate: empty key does not override the global key ──────────────────

/// Sending `XAI_API_KEY: ""` in the authenticate meta must not overwrite the
/// existing global key — the `.filter(|s| !s.is_empty())` guard must prevent
/// it. The subsequent prompt must still succeed using the global key.
#[tokio::test]
async fn authenticate_empty_key_does_not_override_global_key() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Agent has a valid global key.
            let h = Harness::new(); // key = "test-key"

            // Authenticate with an empty key — must be ignored.
            let mut meta = serde_json::Map::new();
            meta.insert("XAI_API_KEY".to_string(), serde_json::json!(""));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("xai-api-key").meta(meta),
                "r.auth",
            );
            h.expect_n_publishes(1).await;

            // Create a session — must pick up the global key, not None.
            let sid = create_session(&h).await;

            // Prompt must succeed (global key still in effect).
            h.http.push(vec![
                XaiEvent::TextDelta {
                    text: "ok".to_string(),
                },
                XaiEvent::Done,
            ]);
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
                "prompt must succeed — empty authenticate must not clear the global key: {val}"
            );
        })
        .await;
}

// ── prompt: session model override passed to HTTP client ──────────────────────

/// After `set_session_model`, the overridden model ID must be passed to
/// `chat_stream` — not the agent's default model.
#[tokio::test]
async fn prompt_uses_session_model_override() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new(); // default model = "grok-3"
            let sid = create_session(&h).await;

            // Override the model for this session.
            let set_model_subj = format!("acp.session.{sid}.agent.set_model");
            h.session_req(
                &set_model_subj,
                SetSessionModelRequest::new(sid.clone(), "grok-3-mini"),
                "r.set_model",
            );
            h.expect_n_publishes(2).await;

            // Prompt — the HTTP client must receive "grok-3-mini", not "grok-3".
            h.http.push(vec![XaiEvent::Done]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hello")]),
                "r.prompt",
            );
            h.expect_n_publishes(3).await;

            let call = h.http.last_call().unwrap();
            assert_eq!(
                call.model, "grok-3-mini",
                "set_session_model must cause the overridden model to be used in chat_stream"
            );
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
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("xai-api-key").meta(meta1),
                "r.auth1",
            );
            h.expect_n_publishes(1).await;

            // Second authenticate with the key that must be consumed by new_session.
            let mut meta2 = serde_json::Map::new();
            meta2.insert("XAI_API_KEY".to_string(), serde_json::json!("second-key"));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("xai-api-key").meta(meta2),
                "r.auth2",
            );
            h.expect_n_publishes(2).await;

            // new_session consumes the pending key (second one).
            let sid = create_session(&h).await;

            // A prompt must succeed — meaning the second key was used, not the first.
            h.http.push(vec![
                XaiEvent::TextDelta {
                    text: "ok".to_string(),
                },
                XaiEvent::Done,
            ]);
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

// ── ENV_MUTEX: serialize tests that touch environment variables ───────────────
//
// `std::env::set_var` is not thread-safe when tests run in parallel. This mutex
// serializes any test that reads or writes env vars so only one such test runs
// at a time within this binary. The lock is held only during `Harness::new()`,
// which is where `XaiAgent::with_deps` reads env vars into struct fields.
static ENV_MUTEX: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
fn env_mutex() -> &'static std::sync::Mutex<()> {
    ENV_MUTEX.get_or_init(|| std::sync::Mutex::new(()))
}

// ── system prompt: prepended to input on every full-history turn ──────────────

/// When `XAI_SYSTEM_PROMPT` is set, `build_input` must prepend a system-role
/// item before history and the user message. The item must use role "system"
/// and carry the exact prompt text.
#[tokio::test]
async fn system_prompt_prepended_to_input() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Hold env mutex only during agent construction so the env var is
            // read by `XaiAgent::with_deps` before any other test can clear it.
            let h = {
                let _env_guard = env_mutex().lock().unwrap();
                // SAFETY: guarded by ENV_MUTEX; only read during with_deps().
                unsafe { std::env::set_var("XAI_SYSTEM_PROMPT", "You are a pirate.") };
                let h = Harness::new();
                unsafe { std::env::remove_var("XAI_SYSTEM_PROMPT") };
                h
            };

            let sid = create_session(&h).await;
            h.http.push(vec![XaiEvent::Done]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("ahoy!")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;

            let call = h.http.last_call().unwrap();
            assert_eq!(
                call.input_len, 2,
                "first turn with system prompt: input must be [system, user] = 2 items"
            );
            assert_eq!(
                call.inputs[0].role, "system",
                "first item must have role 'system'"
            );
            assert_eq!(
                call.inputs[0].content, "You are a pirate.",
                "system item content must match XAI_SYSTEM_PROMPT"
            );
            assert_eq!(
                call.inputs[1].role, "user",
                "second item must be the user message"
            );
        })
        .await;
}

// ── system prompt: NOT sent when previous_response_id shortcut is active ──────

/// When the agent has a cached `previous_response_id`, `prompt()` skips
/// `build_input` entirely and sends only the new user message. The system
/// prompt must not appear in that single-item input array.
#[tokio::test]
async fn system_prompt_not_sent_when_previous_response_id_cached() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = {
                let _env_guard = env_mutex().lock().unwrap();
                unsafe { std::env::set_var("XAI_SYSTEM_PROMPT", "Always be brief.") };
                let h = Harness::new();
                unsafe { std::env::remove_var("XAI_SYSTEM_PROMPT") };
                h
            };

            let sid = create_session(&h).await;
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");

            // Turn 1: server returns a ResponseId; agent caches it.
            h.http.push(vec![
                XaiEvent::ResponseId {
                    id: "resp-abc".to_string(),
                },
                XaiEvent::Done,
            ]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("first")]),
                "r.p1",
            );
            h.expect_n_publishes(2).await;

            // Turn 2: previous_response_id shortcut — build_input is skipped.
            // Input must be exactly one user item; no system item.
            h.http.push(vec![XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("second")]),
                "r.p2",
            );
            h.expect_n_publishes(3).await;

            let call = h.http.last_call().unwrap();
            assert_eq!(
                call.previous_response_id.as_deref(),
                Some("resp-abc"),
                "turn 2 must use the cached response ID"
            );
            assert_eq!(
                call.input_len, 1,
                "with previous_response_id, only the new user message is sent"
            );
            assert_eq!(
                call.inputs[0].role, "user",
                "the single input item must be the user message — not the system prompt"
            );
        })
        .await;
}

// ── close_session mid-stream: session is permanently removed ─────────────────

/// After `close_session` fires while a prompt is streaming, the session must be
/// permanently gone. A subsequent `load_session` on the same ID must return an
/// ACP error — not stale state left behind by the cancel path.
#[tokio::test]
async fn close_session_mid_stream_removes_session_permanently() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Slow stream — blocks until cancelled.
            h.http.push_slow(XaiEvent::TextDelta { text: "partial".to_string() });
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );

            // Let the prompt enter its streaming loop before closing.
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }

            // Close while streaming — sends cancel + removes session from state.
            let close_subj = format!("acp.session.{sid}.agent.close");
            h.session_req(&close_subj, CloseSessionRequest::new(sid.clone()), "r.close");

            // Both prompt response and close response must be published.
            h.expect_n_publishes(3).await;

            // The session must be permanently gone: load_session must return an error.
            let load_subj = format!("acp.session.{sid}.agent.load");
            h.session_req(&load_subj, LoadSessionRequest::new(sid.clone(), "/tmp"), "r.load");
            let payloads = h.expect_n_publishes(4).await;
            let val: serde_json::Value = serde_json::from_slice(payloads.last().unwrap()).unwrap();
            assert!(
                val.get("code").is_some(),
                "load_session after close must return ACP error — session must be fully removed: {val}"
            );
        })
        .await;
}

// ── cancel: no-op after prompt already completed ──────────────────────────────

/// `cancel` sent after a prompt has already finished and published its response
/// is a no-op. The cancel_senders entry is removed when `prompt()` completes,
/// so the cancel notification finds nothing to fire and must not trigger any
/// new publishes.
#[tokio::test]
async fn cancel_after_prompt_completed_is_noop() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Complete a normal prompt.
            h.http.push(vec![
                XaiEvent::TextDelta {
                    text: "done".to_string(),
                },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await; // session + prompt

            // Cancel the same session after the prompt has already finished.
            let cancel_subj = format!("acp.session.{sid}.agent.cancel");
            h.session_notify(&cancel_subj, CancelNotification::new(sid.clone()));

            // Must not trigger any new publishes — the cancel sender is already gone.
            h.expect_no_new_publishes(2).await;
        })
        .await;
}

// ── prompt: empty ContentBlock array completes without error ─────────────────

/// `PromptRequest` with a completely empty `ContentBlock` array must not panic.
/// `user_input` resolves to an empty string, the agent logs a warning and
/// proceeds — a valid `PromptResponse` must be published.
#[tokio::test]
async fn prompt_with_empty_content_blocks_completes() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![XaiEvent::Done]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![]),
                "r.prompt",
            );

            let payloads = h.expect_n_publishes(2).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                val.get("code").is_none(),
                "empty content blocks must produce a valid PromptResponse, not an ACP error: {val}"
            );
        })
        .await;
}

// ── fork: prompt on closed fork returns ACP error ────────────────────────────

/// After a forked session is immediately closed, prompting it must return an
/// ACP error — the session was removed from state by `close_session`.
#[tokio::test]
async fn prompt_on_closed_fork_returns_acp_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Fork the session.
            let fork_subj = format!("acp.session.{sid}.agent.fork");
            h.session_req(
                &fork_subj,
                ForkSessionRequest::new(sid.clone(), "/fork"),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(2).await;
            let fork_val: serde_json::Value = serde_json::from_slice(&payloads[1]).unwrap();
            let fork_id = fork_val["sessionId"].as_str().unwrap().to_string();

            // Close the fork immediately.
            let close_subj = format!("acp.session.{fork_id}.agent.close");
            h.session_req(
                &close_subj,
                CloseSessionRequest::new(fork_id.clone()),
                "r.close",
            );
            h.expect_n_publishes(3).await;

            // Prompt the now-closed fork — must return an ACP error.
            let prompt_subj = format!("acp.session.{fork_id}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(fork_id.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(4).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[3]).unwrap();
            assert!(
                val.get("code").is_some(),
                "prompt on closed fork must return ACP error, got: {val}"
            );
        })
        .await;
}

// ── authenticate: meta without XAI_API_KEY does not set pending key ───────────

/// `authenticate` with meta that does NOT contain `XAI_API_KEY` must leave
/// `pending_api_key` as `None`. The subsequent `new_session` must fall back to
/// the agent-wide global key, and prompts must succeed normally.
#[tokio::test]
async fn authenticate_with_meta_without_xai_api_key_does_not_set_pending_key() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new(); // global key = "test-key"

            // Authenticate with meta that has a different field — NOT XAI_API_KEY.
            let mut meta = serde_json::Map::new();
            meta.insert(
                "SOME_OTHER_FIELD".to_string(),
                serde_json::json!("irrelevant"),
            );
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("xai-api-key").meta(meta),
                "r.auth",
            );
            h.expect_n_publishes(1).await;

            // Create session — global key must be used (pending was never set).
            let sid = create_session(&h).await;

            // Prompt must succeed — global key is in effect.
            h.http.push(vec![
                XaiEvent::TextDelta {
                    text: "ok".to_string(),
                },
                XaiEvent::Done,
            ]);
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
                "prompt must succeed — global key must be used when meta lacks XAI_API_KEY: {val}"
            );
        })
        .await;
}

// ── three-turn ResponseId caching: id updated each turn ──────────────────────

/// After each turn that returns a new ResponseId, the cached id must be updated
/// so the NEXT turn uses the LATEST id — not the one from the first turn.
/// Turn 1 → caches "resp-1"; Turn 2 uses "resp-1", returns "resp-2";
/// Turn 3 must use "resp-2" (not "resp-1").
#[tokio::test]
async fn three_turn_sequence_response_id_updated_each_turn() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");

            // Turn 1: returns resp-1.
            h.http.push(vec![
                XaiEvent::ResponseId {
                    id: "resp-1".to_string(),
                },
                XaiEvent::Done,
            ]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q1")]),
                "r.p1",
            );
            h.expect_n_publishes(2).await;

            // Turn 2: must use resp-1, returns resp-2.
            h.http.push(vec![
                XaiEvent::ResponseId {
                    id: "resp-2".to_string(),
                },
                XaiEvent::Done,
            ]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q2")]),
                "r.p2",
            );
            h.expect_n_publishes(3).await;
            assert_eq!(
                h.http.last_call().unwrap().previous_response_id.as_deref(),
                Some("resp-1"),
                "turn 2 must use resp-1"
            );

            // Turn 3: must use resp-2 (the updated id), not resp-1.
            h.http.push(vec![XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q3")]),
                "r.p3",
            );
            h.expect_n_publishes(4).await;
            assert_eq!(
                h.http.last_call().unwrap().previous_response_id.as_deref(),
                Some("resp-2"),
                "turn 3 must use the updated resp-2, not stale resp-1"
            );
        })
        .await;
}

// ── fork: source closed before fork returns ACP error ────────────────────────

/// Attempting to fork a session that has already been closed must return an
/// ACP error. Only the reverse (fork then close) was previously tested.
#[tokio::test]
async fn fork_of_closed_session_returns_acp_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Close the session first.
            let close_subj = format!("acp.session.{sid}.agent.close");
            h.session_req(
                &close_subj,
                CloseSessionRequest::new(sid.clone()),
                "r.close",
            );
            h.expect_n_publishes(2).await;

            // Attempt to fork the now-closed session.
            let fork_subj = format!("acp.session.{sid}.agent.fork");
            h.session_req(
                &fork_subj,
                ForkSessionRequest::new(sid.clone(), "/fork"),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(3).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[2]).unwrap();
            assert!(
                val.get("code").is_some(),
                "fork of a closed session must return an ACP error, got: {val}"
            );
        })
        .await;
}

// ── fork: no model override falls back to agent default on prompt ─────────────

/// A fork of a session with no model override (model = None) must also have
/// no override. When prompted, the agent default model ("grok-3") must be used.
#[tokio::test]
async fn fork_with_no_model_override_uses_agent_default_on_prompt() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new(); // default model = "grok-3"
            let sid = create_session(&h).await; // no model override

            // Fork the session (source has no override).
            let fork_subj = format!("acp.session.{sid}.agent.fork");
            h.session_req(
                &fork_subj,
                ForkSessionRequest::new(sid.clone(), "/fork"),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(2).await;
            let fork_val: serde_json::Value = serde_json::from_slice(&payloads[1]).unwrap();
            let fork_id = fork_val["sessionId"].as_str().unwrap().to_string();

            // Prompt the fork — agent default model must be used.
            h.http.push(vec![XaiEvent::Done]);
            let prompt_subj = format!("acp.session.{fork_id}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(fork_id.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );
            h.expect_n_publishes(3).await;

            assert_eq!(
                h.http.last_call().unwrap().model,
                "grok-3",
                "fork with no model override must use the agent default model"
            );
        })
        .await;
}

// ── prompt: Error clears history (no partial text saved) ─────────────────────

/// When `XaiEvent::Error` arrives mid-stream, the agent returns Err immediately
/// and skips the history update — session history is empty after the error.
/// The follow-up prompt starts from a clean slate.
#[tokio::test]
async fn prompt_error_mid_stream_partial_text_saved_in_history() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");

            // Stream yields partial text then an error.
            h.http.push(vec![
                XaiEvent::TextDelta { text: "partial answer".to_string() },
                XaiEvent::Error { message: "upstream failure".to_string() },
            ]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("question")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;

            // On the next turn the agent must replay full history (no ResponseId
            // was cached after an error). Input must be: user₁ + assistant₁ + user₂ = 3.
            h.http.push(vec![XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("follow-up")]),
                "r.p2",
            );
            h.expect_n_publishes(3).await;

            let call = h.http.last_call().unwrap();
            // Errors skip history update — session history is empty after error.
            // Follow-up prompt sends only the new user message.
            assert_eq!(
                call.input_len, 1,
                "history must be empty after stream error; follow-up must send only user₂, got input_len = {}",
                call.input_len
            );
        })
        .await;
}

// ── load_session: modes field is populated ────────────────────────────────────

/// `load_session` must return a `modes` field with the current mode ID.
/// Existing tests only verify the `models` field; this locks in that `modes`
/// is also present and populated with the "default" mode.
#[tokio::test]
async fn load_session_returns_modes_field_with_default_mode() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let load_subj = format!("acp.session.{sid}.agent.load");
            h.session_req(
                &load_subj,
                LoadSessionRequest::new(sid.clone(), "/tmp"),
                "r.load",
            );
            let payloads = h.expect_n_publishes(2).await;
            let resp: LoadSessionResponse = serde_json::from_slice(&payloads[1]).unwrap();

            let modes = resp.modes.expect("load_session must return a modes field");
            assert_eq!(
                modes.current_mode_id.to_string(),
                "default",
                "current mode must be 'default'"
            );
            assert!(
                !modes.available_modes.is_empty(),
                "available_modes must be non-empty"
            );
        })
        .await;
}

// ── authenticate → session → authenticate → session: each session gets its key

/// Full double-auth lifecycle: auth(key1) → session1 consumes key1 →
/// auth(key2) → session2 consumes key2. Both sessions must be able to prompt
/// successfully, proving each consumed the correct pending key.
#[tokio::test]
async fn authenticate_new_session_twice_each_session_uses_its_own_key() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // No global key — each session must carry a per-session key.
            let h = Harness::with_api_key("");

            // First auth + session.
            let mut meta1 = serde_json::Map::new();
            meta1.insert(
                "XAI_API_KEY".to_string(),
                serde_json::json!("key-for-session-1"),
            );
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("xai-api-key").meta(meta1),
                "r.auth1",
            );
            h.expect_n_publishes(1).await;
            let sid1 = create_session(&h).await; // consumes key-for-session-1

            // Second auth + session.
            let mut meta2 = serde_json::Map::new();
            meta2.insert(
                "XAI_API_KEY".to_string(),
                serde_json::json!("key-for-session-2"),
            );
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("xai-api-key").meta(meta2),
                "r.auth2",
            );
            h.expect_n_publishes(3).await; // auth1 + session1 + auth2
            let sid2 = create_session(&h).await; // consumes key-for-session-2

            // Prompt session1 — must succeed (has key-for-session-1).
            h.http.push(vec![XaiEvent::Done]);
            h.session_req(
                &format!("acp.session.{sid1}.agent.prompt"),
                PromptRequest::new(sid1.clone(), vec![ContentBlock::from("hi")]),
                "r.p1",
            );
            let payloads = h.expect_n_publishes(5).await; // +session2 +prompt1
            let val1: serde_json::Value = serde_json::from_slice(&payloads[4]).unwrap();
            assert!(
                val1.get("code").is_none(),
                "session1 prompt must succeed: {val1}"
            );

            // Prompt session2 — must also succeed (has key-for-session-2).
            h.http.push(vec![XaiEvent::Done]);
            h.session_req(
                &format!("acp.session.{sid2}.agent.prompt"),
                PromptRequest::new(sid2.clone(), vec![ContentBlock::from("hi")]),
                "r.p2",
            );
            let payloads = h.expect_n_publishes(6).await;
            let val2: serde_json::Value = serde_json::from_slice(&payloads[5]).unwrap();
            assert!(
                val2.get("code").is_none(),
                "session2 prompt must succeed: {val2}"
            );
        })
        .await;
}

// ── SessionNotification: carries the correct session_id ───────────────────────

/// The `SessionNotification` emitted for each `TextDelta` must carry the
/// session_id of the session being prompted — not a stale or default value.
/// `prompt_via_nats_sends_session_notifications` only checks the count; this
/// test inspects the actual session_id field inside the notification.
#[tokio::test]
async fn session_notification_carries_correct_session_id() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::TextDelta {
                    text: "hello".to_string(),
                },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );
            h.expect_n_notifications(1).await;

            let notifications = h.notifier.notifications.lock().unwrap();
            assert_eq!(notifications.len(), 1);
            assert_eq!(
                notifications[0].session_id.to_string(),
                sid,
                "notification must carry the session_id of the prompting session"
            );
        })
        .await;
}

// ── prompt: empty input + cached ResponseId uses shortcut with one empty item ─

/// When `last_response_id` is cached the shortcut path always constructs
/// `vec![InputItem::user(&user_input)]` — exactly 1 item — regardless of
/// whether `user_input` is empty. Even with empty content blocks, one empty
/// user item must be sent (not zero). `prompt_with_empty_content_blocks_completes`
/// exercises the full-history path (no cached id); this test covers the
/// shortcut path specifically.
#[tokio::test]
async fn prompt_empty_input_with_cached_response_id_sends_single_empty_user_item() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");

            // Turn 1: cache a ResponseId.
            h.http.push(vec![
                XaiEvent::ResponseId {
                    id: "resp-x".to_string(),
                },
                XaiEvent::Done,
            ]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.p1",
            );
            h.expect_n_publishes(2).await;

            // Turn 2: empty content blocks + cached id → shortcut path.
            h.http.push(vec![XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![]),
                "r.p2",
            );
            h.expect_n_publishes(3).await;

            let call = h.http.last_call().unwrap();
            assert_eq!(
                call.previous_response_id.as_deref(),
                Some("resp-x"),
                "shortcut path must be active"
            );
            assert_eq!(
                call.input_len, 1,
                "shortcut path with empty input must send exactly one user item"
            );
            assert_eq!(
                call.inputs[0].role, "user",
                "the single item must be role 'user'"
            );
            assert_eq!(
                call.inputs[0].content, "",
                "empty content blocks produce empty content"
            );
        })
        .await;
}

// ── resume_session then prompt ────────────────────────────────────────────────

/// After `resume_session` the session must remain fully functional:
/// a subsequent `prompt` must complete successfully.
#[tokio::test]
async fn resume_session_then_prompt_succeeds() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Resume the session.
            let resume_subj = format!("acp.session.{sid}.agent.resume");
            h.session_req(
                &resume_subj,
                ResumeSessionRequest::new(sid.clone(), "/tmp"),
                "r.resume",
            );
            h.expect_n_publishes(2).await;
            let payloads = h.nats.published_payloads();
            let _: ResumeSessionResponse = serde_json::from_slice(&payloads[1]).unwrap();

            // Now prompt the session — it must still work.
            h.http.push(vec![
                XaiEvent::TextDelta {
                    text: "after resume".to_string(),
                },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hello after resume")]),
                "r.prompt",
            );
            h.expect_n_publishes(3).await;
            let payloads = h.nats.published_payloads();
            // Successful deserialization as PromptResponse (not an ACP error object) is sufficient.
            let _: PromptResponse = serde_json::from_slice(&payloads[2]).unwrap();
        })
        .await;
}

// ── shortcut turns still accumulate history ───────────────────────────────────

/// When `last_response_id` is cached (shortcut path), the HTTP request only
/// sends the new user message — but the history-update block fires regardless.
/// After two shortcut turns the session history should contain all four
/// messages (u1, a1, u2, a2). Forking the session resets `last_response_id` to
/// `None`, so the fork's first prompt uses the full history-replay path.
/// Asserting `input_len == 5` (u1 + a1 + u2 + a2 + new_user) confirms that
/// shortcut turns accumulated history correctly.
#[tokio::test]
async fn prompt_shortcut_turns_still_accumulate_history() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");

            // Turn 1: TextDelta + ResponseId → history: [u1, a1], shortcut active.
            h.http.push(vec![
                XaiEvent::TextDelta { text: "a1".to_string() },
                XaiEvent::ResponseId { id: "id1".to_string() },
                XaiEvent::Done,
            ]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("u1")]),
                "r.p1",
            );
            h.expect_n_publishes(2).await;

            // Turn 2 (shortcut): shortcut sends only new user item to HTTP.
            // History update still fires → history: [u1, a1, u2, a2].
            h.http.push(vec![
                XaiEvent::TextDelta { text: "a2".to_string() },
                XaiEvent::Done,
            ]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("u2")]),
                "r.p2",
            );
            h.expect_n_publishes(3).await;
            // Verify turn 2 used the shortcut path (sent 1 item).
            let call2 = h.http.last_call().unwrap();
            assert_eq!(call2.input_len, 1, "turn 2 must use shortcut (1 item)");

            // Fork the source → resets last_response_id to None.
            let fork_subj = format!("acp.session.{sid}.agent.fork");
            h.session_req(
                &fork_subj,
                ForkSessionRequest::new(sid.clone(), "/fork"),
                "r.fork",
            );
            h.expect_n_publishes(4).await;
            let payloads = h.nats.published_payloads();
            let fork_val: serde_json::Value = serde_json::from_slice(&payloads[3]).unwrap();
            let fork_id = fork_val["sessionId"].as_str().unwrap().to_string();

            // Prompt the fork — must use full history-replay path.
            // Expected input: [u1, a1, u2, a2, new_user] = 5 items.
            h.http.push(vec![XaiEvent::Done]);
            let fork_prompt_subj = format!("acp.session.{fork_id}.agent.prompt");
            h.session_req(
                &fork_prompt_subj,
                PromptRequest::new(fork_id.clone(), vec![ContentBlock::from("u3")]),
                "r.p3",
            );
            h.expect_n_publishes(5).await;

            let call3 = h.http.last_call().unwrap();
            assert_eq!(
                call3.previous_response_id, None,
                "fork resets last_response_id — full-replay path must be used"
            );
            assert_eq!(
                call3.input_len, 5,
                "fork prompt must include all 4 history items from shortcut turns plus the new user item"
            );
        })
        .await;
}

// ── stream timeout returns PromptResponse via NATS ────────────────────────────

/// When the per-chunk inactivity timeout fires the streaming loop must break
/// and publish a valid `PromptResponse` through NATS (not stall forever).
/// Uses `XAI_PROMPT_TIMEOUT_SECS=1` so the test completes in ~1 s.
#[tokio::test]
async fn prompt_stream_timeout_returns_response_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Set a 1-second per-chunk timeout so the test doesn't run for 300 s.
            let h = {
                let _guard = env_lock().lock().unwrap();
                unsafe { std::env::set_var("XAI_PROMPT_TIMEOUT_SECS", "1") };
                let h = Harness::new();
                unsafe { std::env::remove_var("XAI_PROMPT_TIMEOUT_SECS") };
                h
            };

            let sid = create_session(&h).await;

            // Slow response: emits one TextDelta then blocks indefinitely.
            // The per-chunk timeout must fire and break the loop.
            h.http.push_slow(XaiEvent::TextDelta {
                text: "partial".to_string(),
            });

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );

            // The agent must publish a PromptResponse after the 1-second timeout.
            let payloads = h.expect_n_publishes(2).await;
            let _: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
        })
        .await;
}

// ── history trimming caps input at max_history ────────────────────────────────

/// When history exceeds `max_history`, oldest entries are dropped. After two
/// turns with `max_history=2` the history is trimmed to [u2, a2]. The third
/// prompt's `input_len` must be 3 (u2 + a2 + new_user), proving trimming works
/// end-to-end through the NATS/HTTP flow.
#[tokio::test]
async fn history_trimming_caps_input_at_max_history() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Build agent with max_history=2.
            let h = {
                let _guard = env_lock().lock().unwrap();
                unsafe { std::env::set_var("XAI_MAX_HISTORY_MESSAGES", "2") };
                let h = Harness::new();
                unsafe { std::env::remove_var("XAI_MAX_HISTORY_MESSAGES") };
                h
            };

            let sid = create_session(&h).await;
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");

            // Turn 1 → history: [u1, a1] (len=2, no trim yet).
            h.http.push(vec![
                XaiEvent::TextDelta {
                    text: "a1".to_string(),
                },
                XaiEvent::Done,
            ]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("u1")]),
                "r.p1",
            );
            h.expect_n_publishes(2).await;

            // Turn 2 → history grows to [u1, a1, u2, a2] then trimmed to [u2, a2].
            h.http.push(vec![
                XaiEvent::TextDelta {
                    text: "a2".to_string(),
                },
                XaiEvent::Done,
            ]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("u2")]),
                "r.p2",
            );
            h.expect_n_publishes(3).await;

            // Turn 3: full-history path builds input from trimmed history.
            // Expected: [u2, a2, u3] = 3 items.
            h.http.push(vec![XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("u3")]),
                "r.p3",
            );
            h.expect_n_publishes(4).await;

            let call = h.http.last_call().unwrap();
            assert_eq!(
                call.input_len, 3,
                "after trimming to max_history=2, turn 3 must send [u2, a2, u3] = 3 items"
            );
        })
        .await;
}

// ── XAI_MAX_TURNS passed to HTTP client ───────────────────────────────────────

/// `XAI_MAX_TURNS` env var is read at agent construction and must be forwarded
/// as-is to every `chat_stream` call so the xAI Responses API enforces the
/// configured tool-call turn limit.
#[tokio::test]
async fn max_turns_env_var_forwarded_to_http_client() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = {
                let _guard = env_lock().lock().unwrap();
                unsafe { std::env::set_var("XAI_MAX_TURNS", "7") };
                let h = Harness::new();
                unsafe { std::env::remove_var("XAI_MAX_TURNS") };
                h
            };

            let sid = create_session(&h).await;

            h.http.push(vec![XaiEvent::Done]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;

            let call = h.http.last_call().unwrap();
            assert_eq!(
                call.max_turns,
                Some(7),
                "XAI_MAX_TURNS=7 must be forwarded to chat_stream as Some(7)"
            );
        })
        .await;
}

// ── session notification carries AgentMessageChunk with correct text ──────────

/// Each `TextDelta` event must produce a `SessionNotification` whose `update`
/// field is `SessionUpdate::AgentMessageChunk` containing the exact text from
/// the delta. Verifies the wiring at agent.rs lines 527-532.
#[tokio::test]
async fn session_notification_carries_agent_message_chunk_with_text() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::TextDelta {
                    text: "hello world".to_string(),
                },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );
            h.expect_n_notifications(1).await;

            let notifications = h.notifier.notifications.lock().unwrap();
            assert_eq!(notifications.len(), 1);

            match &notifications[0].update {
                SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
                    ContentBlock::Text(t) => assert_eq!(
                        t.text, "hello world",
                        "TextDelta text must be forwarded verbatim in the notification"
                    ),
                    other => panic!("expected ContentBlock::Text, got {other:?}"),
                },
                other => panic!("expected SessionUpdate::AgentMessageChunk, got {other:?}"),
            }
        })
        .await;
}

// ── multiple ContentBlocks joined with newline ────────────────────────────────

/// When a `PromptRequest` contains multiple `ContentBlock::Text` items they
/// must be joined with `"\n"` before being forwarded to `chat_stream`
/// (agent.rs lines 448-456). Verifies the observable HTTP call parameter.
#[tokio::test]
async fn prompt_with_multiple_content_blocks_joins_text_with_newline() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![XaiEvent::Done]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(
                    sid.clone(),
                    vec![ContentBlock::from("hello"), ContentBlock::from("world")],
                ),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;

            let call = h.http.last_call().unwrap();
            // Full-history path: [user_item] — the user item content must be "hello\nworld".
            let user_item = call.inputs.last().unwrap();
            assert_eq!(
                user_item.content, "hello\nworld",
                "multiple text blocks must be joined with '\\n'"
            );
        })
        .await;
}

// ── session branching integration ─────────────────────────────────────────────

/// Fork with branchAtIndex:0 → the fork has an empty history, so the first prompt
/// on the fork sends only the new user message to the xAI HTTP API (no replayed turns).
#[tokio::test]
async fn fork_session_branch_at_index_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await; // publishes: 1

            // Prompt source to add one turn to its history.
            h.http.push(vec![
                XaiEvent::TextDelta { text: "reply".to_string() },
                XaiEvent::Done,
            ]);
            h.session_req(
                &format!("acp.session.{sid}.agent.prompt"),
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt1",
            );
            h.expect_n_publishes(2).await; // publishes: 2

            // Fork with branchAtIndex:0 → empty history.
            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({ "branchAtIndex": 0 }),
            )
            .unwrap();
            h.session_req(
                &format!("acp.session.{sid}.agent.fork"),
                ForkSessionRequest::new(sid.clone(), "/fork").meta(meta),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(3).await; // publishes: 3
            let fork_val: serde_json::Value = serde_json::from_slice(&payloads[2]).unwrap();
            let fork_id = fork_val["sessionId"].as_str().unwrap().to_string();
            assert!(!fork_id.is_empty(), "fork must return a non-empty session ID");

            // Prompt the fork — history is empty so the xAI API call gets only the new turn.
            h.http.push(vec![
                XaiEvent::TextDelta { text: "fork reply".to_string() },
                XaiEvent::Done,
            ]);
            h.session_req(
                &format!("acp.session.{fork_id}.agent.prompt"),
                PromptRequest::new(fork_id.clone(), vec![ContentBlock::from("first on fork")]),
                "r.prompt2",
            );
            h.expect_n_publishes(4).await; // publishes: 4

            let call = h.http.last_call().unwrap();
            assert_eq!(
                call.inputs.len(),
                1,
                "fork with branchAtIndex:0 must replay no history; expected 1 input, got {}",
                call.inputs.len()
            );
        })
        .await;
}

/// session/list_children ext method returns the direct children of a session.
#[tokio::test]
async fn ext_list_children_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let parent_id = create_session(&h).await; // publishes: 1

            // Fork two direct children.
            h.session_req(
                &format!("acp.session.{parent_id}.agent.fork"),
                ForkSessionRequest::new(parent_id.clone(), "/child1"),
                "r.fork1",
            );
            let payloads = h.expect_n_publishes(2).await; // publishes: 2
            let child1_id = serde_json::from_slice::<serde_json::Value>(&payloads[1])
                .unwrap()["sessionId"]
                .as_str()
                .unwrap()
                .to_string();

            h.session_req(
                &format!("acp.session.{parent_id}.agent.fork"),
                ForkSessionRequest::new(parent_id.clone(), "/child2"),
                "r.fork2",
            );
            let payloads = h.expect_n_publishes(3).await; // publishes: 3
            let child2_id = serde_json::from_slice::<serde_json::Value>(&payloads[2])
                .unwrap()["sessionId"]
                .as_str()
                .unwrap()
                .to_string();

            // Call session/list_children via global ext subject.
            let params_json = format!(r#"{{"sessionId":"{}"}}"#, parent_id);
            let params = serde_json::value::RawValue::from_string(params_json).unwrap();
            h.global(
                "acp.agent.ext.session/list_children",
                ExtRequest::new("session/list_children", params.into()),
                "r.ext",
            );
            let payloads = h.expect_n_publishes(4).await; // publishes: 4

            let resp: serde_json::Value = serde_json::from_slice(&payloads[3]).unwrap();
            let mut children: Vec<String> = resp["children"]
                .as_array()
                .expect("response must have children array")
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            children.sort();
            let mut expected = vec![child1_id, child2_id];
            expected.sort();
            assert_eq!(children, expected, "list_children must return both direct children");
        })
        .await;
}

/// list_sessions response includes parentSessionId in the forked session's _meta.
#[tokio::test]
async fn list_sessions_includes_parent_meta_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let parent_id = create_session(&h).await; // publishes: 1

            h.session_req(
                &format!("acp.session.{parent_id}.agent.fork"),
                ForkSessionRequest::new(parent_id.clone(), "/fork"),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(2).await; // publishes: 2
            let fork_id = serde_json::from_slice::<serde_json::Value>(&payloads[1])
                .unwrap()["sessionId"]
                .as_str()
                .unwrap()
                .to_string();

            h.global("acp.agent.session.list", ListSessionsRequest::new(), "r.list");
            let payloads = h.expect_n_publishes(3).await; // publishes: 3

            let resp: ListSessionsResponse = serde_json::from_slice(&payloads[2]).unwrap();
            let fork_info = resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == fork_id)
                .expect("fork must appear in list_sessions");

            let meta = fork_info.meta.as_ref().expect("fork must have _meta");
            assert_eq!(
                meta.get("parentSessionId").and_then(|v| v.as_str()),
                Some(parent_id.as_str()),
                "fork _meta must include parentSessionId pointing to the source session"
            );
        })
        .await;
}
