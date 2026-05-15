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
    AuthenticateRequest, AuthenticateResponse, BlobResourceContents, CancelNotification,
    CloseSessionRequest, CloseSessionResponse, ContentBlock, EmbeddedResource,
    EmbeddedResourceResource, ExtRequest, ForkSessionRequest, InitializeRequest, InitializeResponse,
    ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, ProtocolVersion,
    ResourceLink, ResumeSessionRequest, ResumeSessionResponse, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, SetSessionModelRequest, SetSessionModelResponse, StopReason,
    TextResourceContents,
};
use async_nats::Message;
use async_trait::async_trait;
use futures::channel::mpsc::UnboundedSender;
use futures_util::StreamExt as _;
use futures_util::stream::{self, LocalBoxStream};
use trogon_nats::mocks::MockNatsClient;
use trogon_xai_runner::{
    AgentConfig, AgentLoading, FinishReason, InputItem, SessionNotifier, SkillLoading, XaiAgent,
    XaiEvent, XaiHttpClient,
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
    pub tools: Vec<trogon_xai_runner::ToolSpec>,
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
        tools: &[trogon_xai_runner::ToolSpec],
        previous_response_id: Option<&str>,
        max_turns: Option<u32>,
    ) -> LocalBoxStream<'static, XaiEvent> {
        self.calls.lock().unwrap().push(HttpCall {
            model: model.to_string(),
            input_len: input.len(),
            inputs: input.to_vec(),
            previous_response_id: previous_response_id.map(str::to_string),
            max_turns,
            tools: tools.to_vec(),
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

    /// Build with a fixed system prompt injected via an inline `AgentLoading` impl.
    /// Useful for verifying that the `with_loaders` path correctly injects the
    /// system prompt into the HTTP request through the NATS wire protocol.
    fn new_with_loader_prompt(system_prompt: &'static str) -> Self {
        struct FixedPromptLoader(&'static str);
        impl AgentLoading for FixedPromptLoader {
            fn load_config<'a>(
                &'a self,
                _: &'a str,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = AgentConfig> + Send + 'a>,
            > {
                let sp = self.0.to_string();
                Box::pin(async move {
                    AgentConfig {
                        skill_ids: vec![],
                        system_prompt: Some(sp),
                        model_id: None,
                    }
                })
            }
        }

        struct NullSkillLoader;
        impl SkillLoading for NullSkillLoader {
            fn load<'a>(
                &'a self,
                _: &'a [String],
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>,
            > {
                Box::pin(std::future::ready(None))
            }
        }

        let nats = MockNatsClient::new();
        let http = TestHttpClient::new();
        let notifier = TestNotifier::new();

        let global_tx = nats.inject_messages();
        let session_tx = nats.inject_messages();

        let http_clone = http.clone();
        let notifier_clone = notifier.clone();

        let agent =
            XaiAgent::with_deps(notifier_clone, "grok-3", "test-key", http_clone).with_loaders(
                "agent-id",
                Arc::new(FixedPromptLoader(system_prompt)),
                Arc::new(NullSkillLoader),
            );

        let prefix = AcpPrefix::new("acp").unwrap();
        let (_, io_task) =
            AgentSideNatsConnection::new(agent, nats.clone(), prefix, |fut| {
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
                call.inputs[0].role(),
                Some("system"),
                "first item must have role 'system'"
            );
            assert_eq!(
                call.inputs[0].content(),
                Some("You are a pirate."),
                "system item content must match XAI_SYSTEM_PROMPT"
            );
            assert_eq!(
                call.inputs[1].role(),
                Some("user"),
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
                call.inputs[0].role(),
                Some("user"),
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
                call.inputs[0].role(),
                Some("user"),
                "the single item must be role 'user'"
            );
            assert_eq!(
                call.inputs[0].content(),
                Some(""),
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
                user_item.content(),
                Some("hello\nworld"),
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

/// list_sessions response includes branchedAtIndex in the forked session's _meta
/// when the fork was created with branchAtIndex set.
#[tokio::test]
async fn list_sessions_includes_branched_at_index_in_meta_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let parent_id = create_session(&h).await; // publishes: 1

            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({ "branchAtIndex": 2 }),
            )
            .unwrap();
            h.session_req(
                &format!("acp.session.{parent_id}.agent.fork"),
                ForkSessionRequest::new(parent_id.clone(), "/fork").meta(meta),
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
                meta.get("branchedAtIndex").and_then(|v| v.as_u64()),
                Some(2),
                "fork _meta must include branchedAtIndex: 2"
            );
        })
        .await;
}

/// Fork with an out-of-bounds branchAtIndex copies the full history — the first
/// prompt on the fork replays all inherited messages.
#[tokio::test]
async fn fork_session_branch_at_index_out_of_bounds_replays_full_history_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await; // publishes: 1

            // Prompt source — adds 1 user + 1 assistant message to history.
            h.http.push(vec![
                XaiEvent::TextDelta { text: "assistant reply".to_string() },
                XaiEvent::Done,
            ]);
            h.session_req(
                &format!("acp.session.{sid}.agent.prompt"),
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("user question")]),
                "r.prompt1",
            );
            h.expect_n_publishes(2).await; // publishes: 2

            // Fork with out-of-bounds branchAtIndex → full history copied.
            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({ "branchAtIndex": 99 }),
            )
            .unwrap();
            h.session_req(
                &format!("acp.session.{sid}.agent.fork"),
                ForkSessionRequest::new(sid.clone(), "/fork").meta(meta),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(3).await; // publishes: 3
            let fork_id = serde_json::from_slice::<serde_json::Value>(&payloads[2])
                .unwrap()["sessionId"]
                .as_str()
                .unwrap()
                .to_string();

            // Prompt the fork — must replay full inherited history.
            h.http.push(vec![XaiEvent::Done]);
            h.session_req(
                &format!("acp.session.{fork_id}.agent.prompt"),
                PromptRequest::new(fork_id.clone(), vec![ContentBlock::from("follow-up")]),
                "r.prompt2",
            );
            h.expect_n_publishes(4).await; // publishes: 4

            let call = h.http.last_call().unwrap();
            assert_eq!(
                call.input_len, 3,
                "out-of-bounds branchAtIndex must replay full history: user + assistant + follow-up = 3"
            );
        })
        .await;
}

/// `ext_method("session/list_children")` must return only direct children,
/// not grandchildren.  For the chain A → B → C, querying A returns [B] and
/// querying B returns [C].
#[tokio::test]
async fn ext_list_children_only_returns_direct_children_not_grandchildren_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let a_id = create_session(&h).await; // publishes: 1

            // Fork A → B.
            h.session_req(
                &format!("acp.session.{a_id}.agent.fork"),
                ForkSessionRequest::new(a_id.clone(), "/b"),
                "r.fork_b",
            );
            let payloads = h.expect_n_publishes(2).await; // publishes: 2
            let b_id = serde_json::from_slice::<serde_json::Value>(&payloads[1])
                .unwrap()["sessionId"]
                .as_str()
                .unwrap()
                .to_string();

            // Fork B → C.
            h.session_req(
                &format!("acp.session.{b_id}.agent.fork"),
                ForkSessionRequest::new(b_id.clone(), "/c"),
                "r.fork_c",
            );
            let payloads = h.expect_n_publishes(3).await; // publishes: 3
            let c_id = serde_json::from_slice::<serde_json::Value>(&payloads[2])
                .unwrap()["sessionId"]
                .as_str()
                .unwrap()
                .to_string();

            // list_children(A) must return only B.
            let params_a = serde_json::value::RawValue::from_string(
                format!(r#"{{"sessionId":"{}"}}"#, a_id),
            )
            .unwrap();
            h.global(
                "acp.agent.ext.session/list_children",
                ExtRequest::new("session/list_children", params_a.into()),
                "r.ext_a",
            );
            let payloads = h.expect_n_publishes(4).await; // publishes: 4
            let resp_a: serde_json::Value = serde_json::from_slice(&payloads[3]).unwrap();
            let children_a: Vec<String> = resp_a["children"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            assert_eq!(children_a, vec![b_id.clone()], "A must have only B as child");

            // list_children(B) must return only C.
            let params_b = serde_json::value::RawValue::from_string(
                format!(r#"{{"sessionId":"{}"}}"#, b_id),
            )
            .unwrap();
            h.global(
                "acp.agent.ext.session/list_children",
                ExtRequest::new("session/list_children", params_b.into()),
                "r.ext_b",
            );
            let payloads = h.expect_n_publishes(5).await; // publishes: 5
            let resp_b: serde_json::Value = serde_json::from_slice(&payloads[4]).unwrap();
            let children_b: Vec<String> = resp_b["children"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            assert_eq!(children_b, vec![c_id.clone()], "B must have only C as child");
        })
        .await;
}

/// `ext_method("session/list_children")` on a root session (never forked)
/// returns an empty children array.
#[tokio::test]
async fn ext_list_children_returns_empty_for_root_session_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let root_id = create_session(&h).await; // publishes: 1

            let params = serde_json::value::RawValue::from_string(
                format!(r#"{{"sessionId":"{}"}}"#, root_id),
            )
            .unwrap();
            h.global(
                "acp.agent.ext.session/list_children",
                ExtRequest::new("session/list_children", params.into()),
                "r.ext",
            );
            let payloads = h.expect_n_publishes(2).await; // publishes: 2

            let resp: serde_json::Value = serde_json::from_slice(&payloads[1]).unwrap();
            assert_eq!(
                resp["children"].as_array().map(Vec::len),
                Some(0),
                "root session must have no children"
            );
        })
        .await;
}

// ── ext_method: unknown method returns ACP error via NATS ─────────────────────

/// Sending an unrecognised `ext_method` name over NATS must produce an ACP
/// error response (`code` field present) — not a panic or silent no-op.
#[tokio::test]
async fn ext_method_unknown_method_returns_acp_error_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let _ = create_session(&h).await; // publishes: 1

            let params =
                serde_json::value::RawValue::from_string(r#"{}"#.to_string()).unwrap();
            h.global(
                "acp.agent.ext.unknown/method",
                ExtRequest::new("unknown/method", params.into()),
                "r.ext",
            );
            let payloads = h.expect_n_publishes(2).await; // publishes: 2

            let val: serde_json::Value = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                val.get("code").is_some(),
                "unknown ext method must return an ACP error with 'code'; got: {val}"
            );
        })
        .await;
}

// ── with_loaders: system prompt from AgentLoader reaches the HTTP client ──────

/// When the agent is configured with `with_loaders`, the system prompt from
/// `AgentLoader` must appear as the first `system`-role item in the HTTP
/// request sent to xAI — verified end-to-end through the NATS wire protocol.
#[tokio::test]
async fn with_loaders_system_prompt_injected_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new_with_loader_prompt("You are a loader-injected assistant.");

            let sid = create_session(&h).await;

            h.http.push(vec![XaiEvent::Done]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hello")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;

            let call = h.http.last_call().expect("HTTP client must have been called");
            let system_item = call
                .inputs
                .iter()
                .find(|i| i.role() == Some("system"))
                .expect("a 'system'-role input must be present when with_loaders is configured");
            assert!(
                system_item
                    .content()
                    .map(|c| c.contains("loader-injected"))
                    .unwrap_or(false),
                "system item must contain the loader-provided prompt text; inputs: {:?}",
                call.inputs
            );
        })
        .await;
}

// ── Incomplete continuation ───────────────────────────────────────────────────

/// `ResponseId` + `Finished(Incomplete)` must trigger a second `chat_stream`
/// call with `previous_response_id` set to that ID. For `max_output_tokens`
/// the continuation input must be empty (the model continues from where it stopped).
#[tokio::test]
async fn incomplete_continuation_triggers_second_http_call_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // First response: partial text, a response ID, then Incomplete.
            h.http.push(vec![
                XaiEvent::ResponseId { id: "resp-partial".to_string() },
                XaiEvent::TextDelta { text: "hello".to_string() },
                XaiEvent::Finished {
                    reason: FinishReason::Incomplete,
                    incomplete_reason: Some("max_output_tokens".to_string()),
                },
            ]);
            // Continuation response: finishes normally.
            h.http.push(vec![
                XaiEvent::TextDelta { text: " world".to_string() },
                XaiEvent::Done,
            ]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("continue this")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;

            let calls = h.http.calls.lock().unwrap().clone();
            assert_eq!(calls.len(), 2, "expected 2 HTTP calls (initial + continuation)");

            let cont = &calls[1];
            assert_eq!(
                cont.previous_response_id.as_deref(),
                Some("resp-partial"),
                "continuation must carry the partial response's ID"
            );
            assert!(
                cont.inputs.is_empty(),
                "max_output_tokens continuation must send empty input; got: {:?}",
                cont.inputs
            );
        })
        .await;
}

/// When `incomplete_reason` is NOT `"max_output_tokens"`, the continuation
/// request must re-send the original user message (not empty input).
#[tokio::test]
async fn incomplete_continuation_non_max_output_tokens_resends_user_input_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // First response: incomplete due to max_turns (not max_output_tokens).
            h.http.push(vec![
                XaiEvent::ResponseId { id: "resp-mt".to_string() },
                XaiEvent::TextDelta { text: "partial".to_string() },
                XaiEvent::Finished {
                    reason: FinishReason::Incomplete,
                    incomplete_reason: Some("max_turns".to_string()),
                },
            ]);
            // Continuation response.
            h.http.push(vec![XaiEvent::Done]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(
                    sid.clone(),
                    vec![ContentBlock::from("original message")],
                ),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;

            let calls = h.http.calls.lock().unwrap().clone();
            assert_eq!(calls.len(), 2, "expected 2 HTTP calls (initial + continuation)");

            let cont = &calls[1];
            assert_eq!(
                cont.previous_response_id.as_deref(),
                Some("resp-mt"),
                "continuation must carry the incomplete response's ID"
            );
            let cont_input = cont
                .inputs
                .iter()
                .filter(|i| i.role() == Some("user"))
                .filter_map(|i| i.content())
                .last()
                .unwrap_or("");
            assert_eq!(
                cont_input, "original message",
                "non-max_output_tokens continuation must re-send the user message; got: {cont_input:?}"
            );
        })
        .await;
}

// ── set_session_config_option: "off" and invalid value ───────────────────────

/// Disabling a tool via `set_session_config_option("off")` must remove it from
/// the `tools` slice forwarded to the HTTP client on the next prompt.
#[tokio::test]
async fn disabled_tool_not_forwarded_to_http_client_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let set_cfg_subj = format!("acp.session.{sid}.agent.set_config_option");
            h.session_req(
                &set_cfg_subj,
                SetSessionConfigOptionRequest::new(sid.clone(), "web_search", "on"),
                "r.cfg1",
            );
            h.expect_n_publishes(2).await;

            h.session_req(
                &set_cfg_subj,
                SetSessionConfigOptionRequest::new(sid.clone(), "web_search", "off"),
                "r.cfg2",
            );
            h.expect_n_publishes(3).await;

            h.http.push(vec![XaiEvent::Done]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hello")]),
                "r.prompt",
            );
            h.expect_n_publishes(4).await;

            let call = h.http.last_call().expect("HTTP client must have been called");
            assert!(
                !call.tools.iter().any(|t| t.name() == "web_search"),
                "web_search must not be forwarded after being disabled; tools: {:?}",
                call.tools.iter().map(|t| t.name()).collect::<Vec<_>>()
            );
        })
        .await;
}

/// An invalid config option value (not `"on"` or `"off"`) must return an ACP error.
#[tokio::test]
async fn set_session_config_option_invalid_value_returns_acp_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let set_cfg_subj = format!("acp.session.{sid}.agent.set_config_option");
            h.session_req(
                &set_cfg_subj,
                SetSessionConfigOptionRequest::new(sid.clone(), "web_search", "foo"),
                "r.cfg",
            );
            let payloads = h.expect_n_publishes(2).await;
            let val: serde_json::Value =
                serde_json::from_slice(payloads.last().unwrap()).unwrap();
            assert!(
                val.get("code").is_some(),
                "invalid config option value must return ACP error; got: {val}"
            );
        })
        .await;
}

// ── ContentBlock::ResourceLink ────────────────────────────────────────────────

/// A `ContentBlock::ResourceLink` in the prompt must be converted to
/// `[Resource: name | uri]` text and forwarded to the HTTP client.
#[tokio::test]
async fn resource_link_content_block_forwarded_as_text_via_nats() {
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
                    vec![ContentBlock::ResourceLink(ResourceLink::new(
                        "README",
                        "file:///README.md",
                    ))],
                ),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;

            let call = h.http.last_call().expect("HTTP client must have been called");
            let user_text = call
                .inputs
                .iter()
                .filter(|i| i.role() == Some("user"))
                .filter_map(|i| i.content())
                .last()
                .unwrap_or("");
            assert!(
                user_text.contains("README") && user_text.contains("file:///README.md"),
                "ResourceLink must be forwarded as text; input: {user_text:?}"
            );
        })
        .await;
}

/// `FinishReason::Other` (unknown status string) must be treated as end-of-turn —
/// a valid `PromptResponse` must be published, not an error.
#[tokio::test]
async fn prompt_finished_other_status_treated_as_end_of_turn_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::TextDelta { text: "partial".to_string() },
                XaiEvent::Finished {
                    reason: FinishReason::Other("some_future_status".to_string()),
                    incomplete_reason: None,
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

/// A `ContentBlock::Resource(TextResourceContents)` in the prompt must have its
/// text content forwarded to the HTTP client directly (no wrapping).
#[tokio::test]
async fn embedded_text_resource_content_forwarded_via_nats() {
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
                    vec![ContentBlock::Resource(EmbeddedResource::new(
                        EmbeddedResourceResource::TextResourceContents(
                            TextResourceContents::new("fn main() {}", "file:///src/main.rs"),
                        ),
                    ))],
                ),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;

            let call = h.http.last_call().expect("HTTP client must have been called");
            let user_text = call
                .inputs
                .iter()
                .filter(|i| i.role() == Some("user"))
                .filter_map(|i| i.content())
                .last()
                .unwrap_or("");
            assert_eq!(
                user_text, "fn main() {}",
                "TextResourceContents text must be forwarded directly; got: {user_text:?}"
            );
        })
        .await;
}

/// Enabling `web_search` via `set_session_config_option` causes the tool to
/// appear in the `tools` slice forwarded to the HTTP client on the next prompt.
#[tokio::test]
async fn enabled_tool_forwarded_to_http_client_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Enable web_search for this session.
            let set_cfg_subj = format!("acp.session.{sid}.agent.set_config_option");
            h.session_req(
                &set_cfg_subj,
                SetSessionConfigOptionRequest::new(sid.clone(), "web_search", "on"),
                "r.cfg",
            );
            h.expect_n_publishes(2).await;

            // Prompt — the HTTP call must include web_search in tools.
            h.http.push(vec![XaiEvent::Done]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("search something")]),
                "r.prompt",
            );
            h.expect_n_publishes(3).await;

            let call = h.http.last_call().expect("HTTP client must have been called");
            assert!(
                call.tools.iter().any(|t| t.name() == "web_search"),
                "web_search must be forwarded to HTTP client after being enabled; tools: {:?}",
                call.tools.iter().map(|t| t.name()).collect::<Vec<_>>()
            );
        })
        .await;
}

// ── authenticate: error paths ─────────────────────────────────────────────────

/// `authenticate` with an unrecognised `method_id` must return an ACP error.
#[tokio::test]
async fn authenticate_unknown_method_returns_acp_error_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("oauth2-mystery"),
                "r.auth",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "unknown auth method must return ACP error; got: {val}"
            );
        })
        .await;
}

/// `authenticate` with method `"agent"` on an agent that has no global API key
/// must return an ACP error — the "agent" method is only available when a
/// server-wide key is configured.
#[tokio::test]
async fn authenticate_agent_method_without_server_key_returns_acp_error_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Empty key → no global API key configured.
            let h = Harness::with_api_key("");
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("agent"),
                "r.auth",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "'agent' auth without server key must return ACP error; got: {val}"
            );
        })
        .await;
}

// ── set_session_mode: unknown mode ────────────────────────────────────────────

/// `set_session_mode` with a mode_id other than `"default"` must return an ACP
/// error — the agent only supports the "default" mode.
#[tokio::test]
async fn set_session_mode_unknown_mode_returns_acp_error_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let set_mode_subj = format!("acp.session.{sid}.agent.set_mode");
            h.session_req(
                &set_mode_subj,
                SetSessionModeRequest::new(sid.clone(), "restricted"),
                "r.mode",
            );
            let payloads = h.expect_n_publishes(2).await;
            let val: serde_json::Value = serde_json::from_slice(payloads.last().unwrap()).unwrap();
            assert!(
                val.get("code").is_some(),
                "unknown mode must return ACP error; got: {val}"
            );
        })
        .await;
}

// ── authenticate: key validation errors ──────────────────────────────────────

/// `authenticate` with method `"xai-api-key"` but no `XAI_API_KEY` field in
/// meta (or a wrong field name) must return an ACP error with "XAI_API_KEY
/// missing" in the message.
#[tokio::test]
async fn authenticate_missing_key_in_meta_returns_acp_error_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::with_api_key("");

            // No meta at all.
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("xai-api-key"),
                "r.auth1",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "missing XAI_API_KEY must return ACP error; got: {val}"
            );
            let msg = val["message"].as_str().unwrap_or("");
            assert!(
                msg.contains("XAI_API_KEY"),
                "error message must mention XAI_API_KEY; got: {msg}"
            );

            // Wrong key name in meta.
            let mut bad_meta = serde_json::Map::new();
            bad_meta.insert("WRONG_KEY".to_string(), serde_json::json!("value"));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("xai-api-key").meta(bad_meta),
                "r.auth2",
            );
            let payloads = h.expect_n_publishes(2).await;
            let val2: serde_json::Value = serde_json::from_slice(payloads.last().unwrap()).unwrap();
            assert!(
                val2.get("code").is_some(),
                "wrong meta key must return ACP error; got: {val2}"
            );
        })
        .await;
}

/// `authenticate` with method `"xai-api-key"` and an empty string as the key
/// value must return an ACP error containing "must not be empty".
#[tokio::test]
async fn authenticate_empty_key_returns_acp_error_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::with_api_key("");
            let mut meta = serde_json::Map::new();
            meta.insert("XAI_API_KEY".to_string(), serde_json::json!(""));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("xai-api-key").meta(meta),
                "r.auth",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "empty key must return ACP error; got: {val}"
            );
            let msg = val["message"].as_str().unwrap_or("");
            assert!(
                msg.contains("must not be empty"),
                "error must say 'must not be empty'; got: {msg}"
            );
        })
        .await;
}

// ── forked session inherits API key ───────────────────────────────────────────

/// A forked session must inherit the API key of its parent. Authenticate → new
/// session (attaches key) → fork → prompt on fork must succeed without an extra
/// authenticate call.
#[tokio::test]
async fn forked_session_inherits_api_key_from_parent_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::with_api_key(""); // no global key

            // Authenticate to set pending key.
            let mut meta = serde_json::Map::new();
            meta.insert("XAI_API_KEY".to_string(), serde_json::json!("user-key-fork"));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("xai-api-key").meta(meta),
                "r.auth",
            );
            h.expect_n_publishes(1).await;

            // Create session — consumes pending key, attaches it to session.
            let sid = create_session(&h).await;

            // Fork the session.
            let fork_subj = format!("acp.session.{sid}.agent.fork");
            h.session_req(
                &fork_subj,
                ForkSessionRequest::new(sid.clone(), "/fork"),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(3).await;
            let fork_val: serde_json::Value =
                serde_json::from_slice(payloads.last().unwrap()).unwrap();
            let fork_id = fork_val["sessionId"].as_str().unwrap().to_string();

            // Prompt on forked session — must succeed using inherited key.
            h.http.push(vec![XaiEvent::TextDelta { text: "hi".to_string() }, XaiEvent::Done]);
            let fork_prompt_subj = format!("acp.session.{fork_id}.agent.prompt");
            h.session_req(
                &fork_prompt_subj,
                PromptRequest::new(fork_id.clone(), vec![ContentBlock::from("hello")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(4).await;
            let resp: PromptResponse = serde_json::from_slice(payloads.last().unwrap()).unwrap();
            assert_eq!(
                resp.stop_reason,
                StopReason::EndTurn,
                "fork prompt must succeed using inherited key"
            );
        })
        .await;
}

// ── UsageUpdate notification ──────────────────────────────────────────────────

/// When the HTTP stream yields `XaiEvent::Usage` the agent must emit a
/// `SessionUpdate::UsageUpdate` notification through the `SessionNotifier`.
#[tokio::test]
async fn usage_update_notification_sent_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::Usage { prompt_tokens: 12, completion_tokens: 7 },
                XaiEvent::TextDelta { text: "hello".to_string() },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );
            // Wait for at least 2 notifications: UsageUpdate + AgentMessageChunk.
            h.expect_n_notifications(2).await;

            let notifs = h.notifier.notifications.lock().unwrap();
            let has_usage = notifs.iter().any(|n| {
                matches!(&n.update, SessionUpdate::UsageUpdate(_))
            });
            assert!(
                has_usage,
                "XaiEvent::Usage must produce a SessionUpdate::UsageUpdate notification; got: {notifs:?}"
            );
        })
        .await;
}

// ── non-bash FunctionCall gets Completed notification ─────────────────────────

/// A `FunctionCall` for a tool that is not bash (no execution backend) followed
/// by `FinishReason::ToolCalls` must result in a `ToolCallUpdate` with status
/// `Completed` in the notifier. The agent ends the turn normally.
#[tokio::test]
async fn non_bash_function_call_gets_completed_notification_via_nats() {
    use agent_client_protocol::ToolCallStatus;
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::ResponseId { id: "resp-tool".to_string() },
                XaiEvent::FunctionCall {
                    call_id: "cid-unknown".to_string(),
                    name: "unknown_tool".to_string(),
                    arguments: "{}".to_string(),
                },
                XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("do something")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;

            // Wait for ToolCallUpdate(Completed) notification.
            h.expect_n_notifications(2).await; // ToolCall(Pending) + ToolCallUpdate(Completed)

            let notifs = h.notifier.notifications.lock().unwrap();
            let completed = notifs.iter().find(|n| {
                if let SessionUpdate::ToolCallUpdate(u) = &n.update {
                    u.tool_call_id.to_string() == "cid-unknown"
                        && u.fields.status == Some(ToolCallStatus::Completed)
                } else {
                    false
                }
            });
            assert!(
                completed.is_some(),
                "non-bash FunctionCall must emit ToolCallUpdate(Completed); got: {notifs:?}"
            );
        })
        .await;
}

// ── stale-ID retry ────────────────────────────────────────────────────────────

/// When turn N stored a `response_id` and turn N+1 receives a non-4xx error,
/// the agent must retry transparently:
/// — retry call omits `previous_response_id`
/// — retry call includes full history
/// — final `PromptResponse` is `EndTurn`
#[tokio::test]
async fn stale_id_retry_on_non_4xx_error_succeeds_transparently_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Turn 1: succeeds, stores last_response_id = "resp-t1".
            h.http.push(vec![
                XaiEvent::ResponseId { id: "resp-t1".to_string() },
                XaiEvent::TextDelta { text: "first".to_string() },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("turn 1")]),
                "r.p1",
            );
            h.expect_n_publishes(2).await;

            // Turn 2: first attempt returns a 500-style error → triggers stale-ID retry.
            h.http.push(vec![
                XaiEvent::Error { message: "xAI API error 500: internal server error".to_string() },
            ]);
            // Turn 2, retry: succeeds.
            h.http.push(vec![
                XaiEvent::TextDelta { text: "second".to_string() },
                XaiEvent::Done,
            ]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("turn 2")]),
                "r.p2",
            );
            let payloads = h.expect_n_publishes(3).await;
            let resp: PromptResponse = serde_json::from_slice(payloads.last().unwrap()).unwrap();
            assert_eq!(resp.stop_reason, StopReason::EndTurn, "stale-ID retry must succeed");

            // Verify: 3 HTTP calls total (turn1, turn2-first-attempt, turn2-retry).
            let calls = h.http.calls.lock().unwrap().clone();
            assert_eq!(calls.len(), 3, "expected 3 HTTP calls: turn1 + failed attempt + retry");

            // Turn 2 first attempt must carry previous_response_id.
            assert_eq!(
                calls[1].previous_response_id.as_deref(),
                Some("resp-t1"),
                "first attempt must send previous_response_id"
            );

            // Retry must NOT carry previous_response_id.
            assert!(
                calls[2].previous_response_id.is_none(),
                "stale-ID retry must not send previous_response_id"
            );

            // Retry must include full history (at least 2 messages: user + assistant from turn 1).
            assert!(
                calls[2].inputs.len() >= 2,
                "retry must include full history; got {} inputs",
                calls[2].inputs.len()
            );
        })
        .await;
}

/// A 4xx error must NOT trigger the stale-ID retry. We pre-load exactly one
/// error response — if the agent retried, the empty queue would cause a panic.
#[tokio::test]
async fn api_4xx_error_does_not_trigger_stale_id_retry_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Turn 1: succeeds, stores response_id.
            h.http.push(vec![
                XaiEvent::ResponseId { id: "resp-t1".to_string() },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("turn 1")]),
                "r.p1",
            );
            h.expect_n_publishes(2).await;

            // Turn 2: 401 error — must NOT retry (only one response queued).
            h.http.push(vec![
                XaiEvent::Error { message: "xAI API error 401: unauthorized".to_string() },
            ]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("turn 2")]),
                "r.p2",
            );
            let payloads = h.expect_n_publishes(3).await;
            let val: serde_json::Value = serde_json::from_slice(payloads.last().unwrap()).unwrap();
            assert!(
                val.get("code").is_some(),
                "4xx error must surface as ACP error without retry; got: {val}"
            );

            // Only 2 HTTP calls: turn1 + turn2 (no retry).
            let calls = h.http.calls.lock().unwrap().clone();
            assert_eq!(calls.len(), 2, "4xx must not trigger retry; expected 2 HTTP calls");
        })
        .await;
}

// ── binary blob resource ──────────────────────────────────────────────────────

/// A `ContentBlock::Resource` wrapping `BlobResourceContents` must be forwarded
/// to the HTTP client as a text placeholder `[Binary resource: <uri> (<mime>)]`.
#[tokio::test]
async fn binary_blob_resource_content_forwarded_as_text_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![XaiEvent::TextDelta { text: "ok".to_string() }, XaiEvent::Done]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(
                    sid.clone(),
                    vec![ContentBlock::Resource(EmbeddedResource::new(
                        EmbeddedResourceResource::BlobResourceContents(
                            BlobResourceContents::new("aGVsbG8=", "file:///data.bin")
                                .mime_type("application/octet-stream"),
                        ),
                    ))],
                ),
                "r.blob",
            );
            h.expect_n_publishes(2).await;

            let calls = h.http.calls.lock().unwrap().clone();
            assert_eq!(calls.len(), 1);
            let content = calls[0]
                .inputs
                .iter()
                .filter(|i| i.role() == Some("user"))
                .filter_map(|i| i.content())
                .last()
                .unwrap_or("");
            assert_eq!(
                content,
                "[Binary resource: file:///data.bin (application/octet-stream)]",
                "binary blob must be forwarded as a text placeholder via NATS; got: {content:?}"
            );
        })
        .await;
}

// ── initialize: agent auth method conditional on server key ──────────────────

/// `initialize` must advertise the `"agent"` auth method when a server key is
/// configured, and must NOT advertise it when no key is present.
#[tokio::test]
async fn initialize_advertises_agent_auth_method_only_with_server_key_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // With a server key — "agent" method must appear.
            let h_with_key = Harness::new(); // uses "test-key"
            h_with_key.global(
                "acp.agent.initialize",
                InitializeRequest::new(ProtocolVersion::LATEST),
                "r.init1",
            );
            let payloads = h_with_key.expect_n_publishes(1).await;
            let resp: InitializeResponse = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                resp.auth_methods.iter().any(|m| m.id().to_string() == "agent"),
                "agent with server key must advertise 'agent' auth method; got: {:?}",
                resp.auth_methods
            );
        })
        .await;

    tokio::task::LocalSet::new()
        .run_until(async {
            // Without a server key — "agent" method must NOT appear.
            let h_no_key = Harness::with_api_key("");
            h_no_key.global(
                "acp.agent.initialize",
                InitializeRequest::new(ProtocolVersion::LATEST),
                "r.init2",
            );
            let payloads = h_no_key.expect_n_publishes(1).await;
            let resp: InitializeResponse = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                !resp.auth_methods.iter().any(|m| m.id().to_string() == "agent"),
                "agent without server key must NOT advertise 'agent' auth method; got: {:?}",
                resp.auth_methods
            );
        })
        .await;
}

// ── MAX_CONTINUATIONS exhaustion ──────────────────────────────────────────────

/// After 5 `Incomplete` continuations the agent must give up and return
/// `StopReason::Cancelled`.  We push 6 `[ResponseId, Finished(Incomplete)]`
/// responses (the guard fires on the 6th iteration) and verify the final
/// `PromptResponse` carries `stop_reason == Cancelled`.
#[tokio::test]
async fn max_continuations_exhausted_returns_cancelled_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // 6 incomplete responses — MAX_CONTINUATIONS = 5, guard fires on 6th.
            for i in 0u32..6 {
                h.http.push(vec![
                    XaiEvent::ResponseId { id: format!("resp-{i}") },
                    XaiEvent::Finished {
                        reason: FinishReason::Incomplete,
                        incomplete_reason: Some("max_output_tokens".to_string()),
                    },
                ]);
            }

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("keep going")]),
                "r.cont",
            );
            // new_session reply (1) + prompt reply (1)
            let payloads = h.expect_n_publishes(2).await;
            let resp: PromptResponse = serde_json::from_slice(payloads.last().unwrap()).unwrap();
            assert_eq!(
                resp.stop_reason,
                StopReason::Cancelled,
                "exhausted MAX_CONTINUATIONS must return StopReason::Cancelled via NATS"
            );
        })
        .await;
}

// ── no tools: empty tools array ───────────────────────────────────────────────

/// When no tools are enabled, the `tools` slice passed to `chat_stream` must be
/// empty. Verifies that the agent does not forward an empty-but-allocated vec as
/// a tools list through the NATS wire path.
#[tokio::test]
async fn no_tools_omits_tools_from_http_request_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![XaiEvent::Done]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("plain query")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;

            let call = h.http.last_call().unwrap();
            // Server-side tools are omitted when none are enabled.
            // The standard client-side tools (read_file, write_file, …) are
            // always injected via all_tool_defs() regardless of enabled_tools.
            let server_side: Vec<_> = call
                .tools
                .iter()
                .filter(|t| matches!(t, trogon_xai_runner::ToolSpec::ServerSide(_)))
                .collect();
            assert!(
                server_side.is_empty(),
                "no server-side tools must be present when none are enabled; got: {:?}",
                server_side.iter().map(|t| t.name()).collect::<Vec<_>>()
            );
            assert!(
                !call.tools.is_empty(),
                "standard client-side tools must always be present"
            );
        })
        .await;
}

// ── fork inherits enabled tools ───────────────────────────────────────────────

/// A forked session must inherit the tool configuration of the source session.
/// Enable `web_search` on the source, fork it, then prompt the fork — the HTTP
/// call must include `web_search` in the tools array.
#[tokio::test]
async fn fork_session_inherits_enabled_tools_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Enable web_search on source session.
            let set_cfg_subj = format!("acp.session.{sid}.agent.set_config_option");
            h.session_req(
                &set_cfg_subj,
                SetSessionConfigOptionRequest::new(sid.clone(), "web_search", "on"),
                "r.cfg",
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
            let fork_val: serde_json::Value =
                serde_json::from_slice(payloads.last().unwrap()).unwrap();
            let fork_id = fork_val["sessionId"].as_str().unwrap().to_string();

            // Prompt the forked session.
            h.http.push(vec![XaiEvent::Done]);
            let fork_prompt_subj = format!("acp.session.{fork_id}.agent.prompt");
            h.session_req(
                &fork_prompt_subj,
                PromptRequest::new(fork_id.clone(), vec![ContentBlock::from("search this")]),
                "r.prompt",
            );
            h.expect_n_publishes(4).await;

            let call = h.http.last_call().unwrap();
            assert!(
                call.tools.iter().any(|t| t.name() == "web_search"),
                "forked session must inherit web_search tool from source; got tools: {:?}",
                call.tools.iter().map(|t| t.name()).collect::<Vec<_>>()
            );
        })
        .await;
}

// ── set_session_model clears cached response ID ───────────────────────────────

/// After a prompt that stores a `ResponseId`, calling `set_session_model` must
/// clear the cached ID. The next prompt must use the full-history path (no
/// `previous_response_id`) so it doesn't carry a stale ID from a prior model.
#[tokio::test]
async fn set_session_model_clears_last_response_id_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");

            // Turn 1: server returns a ResponseId — agent caches it.
            h.http.push(vec![
                XaiEvent::ResponseId { id: "resp-model-test".to_string() },
                XaiEvent::Done,
            ]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hello")]),
                "r.p1",
            );
            h.expect_n_publishes(2).await;

            // Change the model — must clear the cached response ID.
            let set_model_subj = format!("acp.session.{sid}.agent.set_model");
            h.session_req(
                &set_model_subj,
                SetSessionModelRequest::new(sid.clone(), "grok-3-mini"),
                "r.model",
            );
            h.expect_n_publishes(3).await;

            // Turn 2: must NOT use previous_response_id.
            h.http.push(vec![XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("world")]),
                "r.p2",
            );
            h.expect_n_publishes(4).await;

            let calls = h.http.calls.lock().unwrap().clone();
            assert_eq!(calls.len(), 2);
            assert_eq!(
                calls[1].previous_response_id,
                None,
                "set_session_model must clear the cached response ID; turn 2 must not send previous_response_id"
            );
        })
        .await;
}

// ── fork inherits system prompt ───────────────────────────────────────────────

/// A forked session must send the loader's system prompt as the first input
/// item on its first full-history prompt. The fork clears the parent's cached
/// `ResponseId`, forcing it through `build_full_history_input`.
#[tokio::test]
async fn fork_session_inherits_system_prompt_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new_with_loader_prompt("Be concise.");
            let sid = create_session(&h).await;
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");

            // Parent turn: no ResponseId so history is built from scratch.
            h.http.push(vec![
                XaiEvent::TextDelta { text: "parent reply".to_string() },
                XaiEvent::Done,
            ]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("parent question")]),
                "r.p1",
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
            let fork_val: serde_json::Value =
                serde_json::from_slice(payloads.last().unwrap()).unwrap();
            let fork_id = fork_val["sessionId"].as_str().unwrap().to_string();

            // Fork's first prompt: no ResponseId → full history with system prompt.
            h.http.push(vec![XaiEvent::Done]);
            let fork_prompt_subj = format!("acp.session.{fork_id}.agent.prompt");
            h.session_req(
                &fork_prompt_subj,
                PromptRequest::new(fork_id.clone(), vec![ContentBlock::from("fork question")]),
                "r.p2",
            );
            h.expect_n_publishes(4).await;

            let call = h.http.last_call().unwrap();
            assert_eq!(
                call.inputs[0].role(),
                Some("system"),
                "first input of fork's prompt must be the system role item"
            );
            assert_eq!(
                call.inputs[0].content(),
                Some("Be concise."),
                "system prompt content must match the loader's value"
            );
        })
        .await;
}

// ── tool-call-only response does not add assistant to history ─────────────────

/// A response that contains only a `FunctionCall` + `FinishReason::ToolCalls`
/// (no `TextDelta`) must not add an assistant entry to the session history.
/// The next prompt's HTTP input must be `[user_t1, user_t2]` — two items, no
/// assistant in between.
#[tokio::test]
async fn tool_call_response_ends_turn_without_updating_history_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");

            // Turn 1: FunctionCall only — no TextDelta, no ResponseId.
            h.http.push(vec![
                XaiEvent::FunctionCall {
                    call_id: "cid-ws".to_string(),
                    name: "web_search".to_string(),
                    arguments: r#"{"query":"rust"}"#.to_string(),
                },
                XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
                XaiEvent::Done,
            ]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("search")]),
                "r.p1",
            );
            h.expect_n_publishes(2).await;

            // Turn 2: full-history path — expect [user_t1, user_t2], no assistant.
            h.http.push(vec![XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("follow up")]),
                "r.p2",
            );
            h.expect_n_publishes(3).await;

            let call = h.http.last_call().unwrap();
            assert_eq!(
                call.input_len, 2,
                "tool-call-only turn must not save an assistant entry; \
                 turn 2 must see exactly [user_t1, user_t2]"
            );
            assert!(
                call.inputs.iter().all(|i| i.role() != Some("assistant")),
                "no assistant input must appear after a tool-call-only turn; got: {:?}",
                call.inputs.iter().map(|i| i.role()).collect::<Vec<_>>()
            );
        })
        .await;
}

// ── history truncation edge cases ─────────────────────────────────────────────

/// With `max_history=2`, after turn 2 the history `[u1,a1,u2,a2]` exceeds the
/// limit and is trimmed to `[u2,a2]`. Turn 3's HTTP input is `[u2,a2,u3]`
/// (3 items), and the first user item is `"u2"` — confirming the oldest pair
/// was dropped, not the newest.
#[tokio::test]
async fn history_truncation_drops_oldest_pair_first_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = {
                let _guard = env_lock().lock().unwrap();
                unsafe { std::env::set_var("XAI_MAX_HISTORY_MESSAGES", "2") };
                let h = Harness::new();
                unsafe { std::env::remove_var("XAI_MAX_HISTORY_MESSAGES") };
                h
            };

            let sid = create_session(&h).await;
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");

            // Turn 1 → history: [u1, a1] = 2, at limit.
            h.http.push(vec![XaiEvent::TextDelta { text: "a1".to_string() }, XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("u1")]),
                "r.p1",
            );
            h.expect_n_publishes(2).await;

            // Turn 2 → saves [u1,a1,u2,a2] = 4 > 2 → trim to [u2,a2].
            h.http.push(vec![XaiEvent::TextDelta { text: "a2".to_string() }, XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("u2")]),
                "r.p2",
            );
            h.expect_n_publishes(3).await;

            // Turn 3: full-history input is built from trimmed [u2,a2] + user_t3:
            // [u2, a2, u3] = 3 items.
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
                "oldest pair [u1,a1] must be dropped; expected [u2,a2,u3]=3, got {}",
                call.input_len
            );
            let first_user_content = call
                .inputs
                .iter()
                .find(|i| i.role() == Some("user"))
                .and_then(|i| i.content())
                .unwrap_or("");
            assert_eq!(
                first_user_content, "u2",
                "first user input must be u2 (oldest pair dropped), not u1"
            );
        })
        .await;
}

/// With `max_history=4`, exactly 4 stored messages must NOT be trimmed.
/// Turn 3's HTTP input is `[u1, a1, u2, a2, u3]` = 5 items (no trim at boundary).
#[tokio::test]
async fn history_at_exactly_the_limit_is_not_trimmed_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = {
                let _guard = env_lock().lock().unwrap();
                unsafe { std::env::set_var("XAI_MAX_HISTORY_MESSAGES", "4") };
                let h = Harness::new();
                unsafe { std::env::remove_var("XAI_MAX_HISTORY_MESSAGES") };
                h
            };

            let sid = create_session(&h).await;
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");

            // Turn 1 → [u1, a1].
            h.http.push(vec![XaiEvent::TextDelta { text: "a1".to_string() }, XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("u1")]),
                "r.p1",
            );
            h.expect_n_publishes(2).await;

            // Turn 2 → [u1, a1, u2, a2] — exactly at max_history=4, no trim.
            h.http.push(vec![XaiEvent::TextDelta { text: "a2".to_string() }, XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("u2")]),
                "r.p2",
            );
            h.expect_n_publishes(3).await;

            // Turn 3: history is exactly 4 before trimming would fire (trim only when
            // len > max). Input = [u1, a1, u2, a2, u3] = 5 items.
            h.http.push(vec![XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("u3")]),
                "r.p3",
            );
            h.expect_n_publishes(4).await;

            let call = h.http.last_call().unwrap();
            assert_eq!(
                call.input_len, 5,
                "at exactly max_history=4 no trim should occur; expected [u1,a1,u2,a2,u3]=5, got {}",
                call.input_len
            );
        })
        .await;
}

/// With an odd `max_history` (e.g. 3), the trim loop drops pairs while
/// `len > max`, so after turn 2 `[u1,a1,u2,a2]=4 > 3` → drops `[u1,a1]` →
/// `[u2,a2]=2 ≤ 3`. Turn 3's HTTP input is `[u2,a2,u3]` = 3 items —
/// identical to `max_history=2` behaviour (never leaves an incomplete pair).
#[tokio::test]
async fn history_truncation_odd_max_drops_one_pair_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // max=3 (odd): 4 > 3 → drop oldest pair → 2 remain.
            let h = {
                let _guard = env_lock().lock().unwrap();
                unsafe { std::env::set_var("XAI_MAX_HISTORY_MESSAGES", "3") };
                let h = Harness::new();
                unsafe { std::env::remove_var("XAI_MAX_HISTORY_MESSAGES") };
                h
            };

            let sid = create_session(&h).await;
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");

            // Two turns → saves [u1,a1,u2,a2] = 4 > 3 → trim to [u2,a2].
            h.http.push(vec![XaiEvent::TextDelta { text: "a1".to_string() }, XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("u1")]),
                "r.p1",
            );
            h.expect_n_publishes(2).await;

            h.http.push(vec![XaiEvent::TextDelta { text: "a2".to_string() }, XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("u2")]),
                "r.p2",
            );
            h.expect_n_publishes(3).await;

            // Turn 3: full-history input is [u2, a2, u3] = 3 items.
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
                "odd max=3 must trim to 2 stored messages; turn 3 must see [u2,a2,u3]=3, got {}",
                call.input_len
            );
            let first_user = call
                .inputs
                .iter()
                .find(|i| i.role() == Some("user"))
                .and_then(|i| i.content())
                .unwrap_or("");
            assert_eq!(first_user, "u2", "u1 must have been dropped");
        })
        .await;
}

// ── authenticate: agent method uses server key ────────────────────────────────

/// `authenticate` with method `"agent"` on an agent that HAS a server key must
/// succeed (no `code` field in the response) and the resulting session must be
/// able to prompt.
#[tokio::test]
async fn authenticate_agent_method_uses_server_key_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new(); // "test-key" is the configured server key

            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("agent"),
                "r.auth",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_none(),
                "'agent' auth with server key must succeed (no 'code'); got: {val}"
            );

            // Session created after authenticate must be usable.
            let sid = create_session(&h).await;
            h.http
                .push(vec![XaiEvent::TextDelta { text: "ok".to_string() }, XaiEvent::Done]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hello")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(3).await;
            let resp: PromptResponse = serde_json::from_slice(payloads.last().unwrap()).unwrap();
            assert_eq!(
                resp.stop_reason,
                StopReason::EndTurn,
                "session after agent-auth must reach EndTurn"
            );
        })
        .await;
}

// ── authenticate: non-string key in meta ─────────────────────────────────────

/// `authenticate` with `XAI_API_KEY` set to a non-string JSON value (integer 42)
/// must return an ACP error whose message mentions `XAI_API_KEY`.
#[tokio::test]
async fn authenticate_non_string_key_in_meta_returns_acp_error_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::with_api_key("");
            let mut meta = serde_json::Map::new();
            meta.insert("XAI_API_KEY".to_string(), serde_json::json!(42_u64));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("xai-api-key").meta(meta),
                "r.auth",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "non-string XAI_API_KEY must return ACP error; got: {val}"
            );
            let msg = val["message"].as_str().unwrap_or("");
            assert!(
                msg.contains("XAI_API_KEY"),
                "error must mention XAI_API_KEY; got: {msg}"
            );
        })
        .await;
}

// ── pending API key consumed by first new_session ─────────────────────────────

/// The pending API key set by `authenticate` must be consumed by the first
/// `new_session` call. A second session created in the same agent must have no
/// key and its `prompt` must return an ACP error.
#[tokio::test]
async fn pending_api_key_consumed_by_first_new_session_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::with_api_key(""); // no global key

            // Authenticate — sets a pending key.
            let mut meta = serde_json::Map::new();
            meta.insert("XAI_API_KEY".to_string(), serde_json::json!("user-key"));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("xai-api-key").meta(meta),
                "r.auth",
            );
            h.expect_n_publishes(1).await; // auth response

            // sess1 — consumes the pending key.
            let sess1 = create_session(&h).await; // total publishes: 2

            // sess2 — no pending key remaining.
            h.global(
                "acp.agent.session.new",
                NewSessionRequest::new("/tmp"),
                "r.sess2",
            );
            let payloads = h.expect_n_publishes(3).await;
            let v: serde_json::Value = serde_json::from_slice(payloads.last().unwrap()).unwrap();
            let sess2 = v["sessionId"].as_str().unwrap().to_string();

            // Prompt sess2 — must fail immediately (no API key).
            let p2_subj = format!("acp.session.{sess2}.agent.prompt");
            h.session_req(
                &p2_subj,
                PromptRequest::new(sess2.clone(), vec![ContentBlock::from("hello")]),
                "r.p2",
            );
            let payloads = h.expect_n_publishes(4).await;
            let val2: serde_json::Value =
                serde_json::from_slice(payloads.last().unwrap()).unwrap();
            assert!(
                val2.get("code").is_some(),
                "sess2 prompt without key must return ACP error; got: {val2}"
            );

            // Prompt sess1 — must succeed using the key it absorbed.
            h.http
                .push(vec![XaiEvent::TextDelta { text: "reply".to_string() }, XaiEvent::Done]);
            let p1_subj = format!("acp.session.{sess1}.agent.prompt");
            h.session_req(
                &p1_subj,
                PromptRequest::new(sess1.clone(), vec![ContentBlock::from("hello")]),
                "r.p1",
            );
            let payloads = h.expect_n_publishes(5).await;
            let resp: PromptResponse = serde_json::from_slice(payloads.last().unwrap()).unwrap();
            assert_eq!(
                resp.stop_reason,
                StopReason::EndTurn,
                "sess1 must succeed using the consumed pending key"
            );
        })
        .await;
}

// ── session usable after cancel ───────────────────────────────────────────────

/// Cancelling an in-flight prompt must return `Cancelled`; the same session must
/// then accept and complete a second prompt with `EndTurn`.
#[tokio::test]
async fn session_is_usable_after_cancel_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // First prompt: slow — blocks indefinitely after the first event.
            h.http.push_slow(XaiEvent::TextDelta {
                text: "partial".to_string(),
            });
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("first")]),
                "r.p1",
            );

            // Yield so the agent task starts streaming before we cancel.
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }

            // Cancel — fires the abort channel.
            let cancel_subj = format!("acp.session.{sid}.agent.cancel");
            h.session_notify(&cancel_subj, CancelNotification::new(sid.clone()));

            // Wait for the cancelled prompt response (new_session + prompt = 2 publishes).
            let payloads = h.expect_n_publishes(2).await;
            let resp1: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert_eq!(
                resp1.stop_reason,
                StopReason::Cancelled,
                "cancelled prompt must return Cancelled"
            );

            // Second prompt: normal — must complete.
            h.http
                .push(vec![XaiEvent::TextDelta { text: "second reply".to_string() }, XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("second")]),
                "r.p2",
            );
            let payloads = h.expect_n_publishes(3).await;
            let resp2: PromptResponse = serde_json::from_slice(payloads.last().unwrap()).unwrap();
            assert_eq!(
                resp2.stop_reason,
                StopReason::EndTurn,
                "session must be usable after cancel; second prompt must reach EndTurn"
            );
        })
        .await;
}

// ── initialize: xai-api-key always advertised ─────────────────────────────────

/// `initialize` must always include an auth method with id `"xai-api-key"` in
/// `auth_methods`, whether or not a server key is configured.
#[tokio::test]
async fn initialize_always_advertises_xai_api_key_method_via_nats() {
    // With server key.
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.global(
                "acp.agent.initialize",
                InitializeRequest::new(ProtocolVersion::LATEST),
                "r.init1",
            );
            let payloads = h.expect_n_publishes(1).await;
            let resp: InitializeResponse = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                resp.auth_methods.iter().any(|m| m.id().to_string() == "xai-api-key"),
                "agent with server key must advertise 'xai-api-key'; got: {:?}",
                resp.auth_methods
            );
        })
        .await;

    // Without server key.
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::with_api_key("");
            h.global(
                "acp.agent.initialize",
                InitializeRequest::new(ProtocolVersion::LATEST),
                "r.init2",
            );
            let payloads = h.expect_n_publishes(1).await;
            let resp: InitializeResponse = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                resp.auth_methods.iter().any(|m| m.id().to_string() == "xai-api-key"),
                "agent without server key must also advertise 'xai-api-key'; got: {:?}",
                resp.auth_methods
            );
        })
        .await;
}

// ── initialize: embedded_context capability ───────────────────────────────────

/// `initialize` must declare `agent_capabilities.prompt_capabilities.embedded_context = true`.
#[tokio::test]
async fn initialize_declares_embedded_context_prompt_capability_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.global(
                "acp.agent.initialize",
                InitializeRequest::new(ProtocolVersion::LATEST),
                "r.init",
            );
            let payloads = h.expect_n_publishes(1).await;
            let resp: InitializeResponse = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                resp.agent_capabilities.prompt_capabilities.embedded_context,
                "agent must declare embedded_context=true via NATS"
            );
        })
        .await;
}

// ── list_sessions before any session ─────────────────────────────────────────

/// `list_sessions` before any session is created must return an empty sessions list.
#[tokio::test]
async fn list_sessions_is_empty_before_any_session_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.global(
                "acp.agent.session.list",
                ListSessionsRequest::new(),
                "r.list",
            );
            let payloads = h.expect_n_publishes(1).await;
            let resp: ListSessionsResponse = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                resp.sessions.is_empty(),
                "sessions must be empty before any session is created; got: {:?}",
                resp.sessions
            );
        })
        .await;
}

// ── new_session: config_options exposed as off ────────────────────────────────

/// `new_session` must return `config_options` with `web_search` and `x_search`
/// both set to `"off"` by default.
#[tokio::test]
async fn new_session_exposes_all_tool_config_options_as_off_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.global(
                "acp.agent.session.new",
                NewSessionRequest::new("/tmp"),
                "r.new",
            );
            let payloads = h.expect_n_publishes(1).await;
            // Inspect via raw JSON to avoid importing SessionConfigKind.
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            let opts = val["configOptions"]
                .as_array()
                .expect("configOptions must be present in new_session response");
            assert_eq!(opts.len(), 2, "expected 2 tool toggles; got: {opts:?}");
            let ids: Vec<&str> = opts.iter().filter_map(|o| o["id"].as_str()).collect();
            assert!(
                ids.contains(&"web_search"),
                "web_search must be in configOptions; got: {ids:?}"
            );
            assert!(
                ids.contains(&"x_search"),
                "x_search must be in configOptions; got: {ids:?}"
            );
            for opt in opts {
                let current = opt["currentValue"].as_str().unwrap_or_default();
                assert_eq!(
                    current,
                    "off",
                    "option '{}' must default to 'off'",
                    opt["id"].as_str().unwrap_or("?")
                );
            }
        })
        .await;
}

// ── multiple tool calls in one turn ──────────────────────────────────────────

/// Two `FunctionCall` events in a single response must not cause a panic.
/// The agent must return `EndTurn` (no execution backend, so no tool loop).
#[tokio::test]
async fn multiple_tool_calls_in_one_turn_do_not_panic_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::ResponseId {
                    id: "resp_tools".to_string(),
                },
                XaiEvent::FunctionCall {
                    call_id: "call_a".to_string(),
                    name: "web_search".to_string(),
                    arguments: "{\"q\":\"rust\"}".to_string(),
                },
                XaiEvent::FunctionCall {
                    call_id: "call_b".to_string(),
                    name: "get_weather".to_string(),
                    arguments: "{\"city\":\"London\"}".to_string(),
                },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("use two tools")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(2).await;
            let resp: PromptResponse = serde_json::from_slice(payloads.last().unwrap()).unwrap();
            assert_eq!(
                resp.stop_reason,
                StopReason::EndTurn,
                "two FunctionCall events in one turn must not panic and must return EndTurn"
            );
        })
        .await;
}

// ── text assembled from multiple deltas ──────────────────────────────────────

/// Multiple `TextDelta` events in one response must each produce an
/// `AgentMessageChunk` notification and the final `PromptResponse` must have
/// `stop_reason == EndTurn`.
#[tokio::test]
async fn text_is_assembled_from_multiple_deltas_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::TextDelta {
                    text: "Hel".to_string(),
                },
                XaiEvent::TextDelta {
                    text: "lo".to_string(),
                },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(2).await;
            let resp: PromptResponse = serde_json::from_slice(payloads.last().unwrap()).unwrap();
            assert_eq!(
                resp.stop_reason,
                StopReason::EndTurn,
                "multiple TextDelta events must assemble into EndTurn response"
            );

            // Each TextDelta must produce one AgentMessageChunk notification.
            h.expect_n_notifications(2).await;
            assert_eq!(
                h.notifier.count(),
                2,
                "each TextDelta must produce exactly one AgentMessageChunk notification"
            );
        })
        .await;
}

// ── tool call + response.failed compensates history ──────────────────────────

/// A `FunctionCall` event followed by `FinishReason::Failed` must cause the
/// agent to return an ACP error. The error response must have a `code` field.
#[tokio::test]
async fn tool_call_then_response_failed_compensates_history_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::ResponseId {
                    id: "resp_err".to_string(),
                },
                XaiEvent::FunctionCall {
                    call_id: "call_1".to_string(),
                    name: "web_search".to_string(),
                    arguments: "{}".to_string(),
                },
                XaiEvent::Finished {
                    reason: FinishReason::Failed,
                    incomplete_reason: None,
                },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("search something")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(2).await;
            let val: serde_json::Value = serde_json::from_slice(payloads.last().unwrap()).unwrap();
            assert!(
                val.get("code").is_some(),
                "FunctionCall + FinishReason::Failed must return ACP error with 'code'; got: {val}"
            );
        })
        .await;
}

// ── fork histories are independent after diverging prompts ────────────────────

/// After forking a session and prompting each branch independently, the HTTP
/// call inputs for each branch must contain only that branch's message.
/// Neither branch must have the other branch's message in its inputs.
#[tokio::test]
async fn fork_histories_are_independent_after_diverging_prompts_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Shared prompt on parent — no ResponseId so history is stored.
            h.http.push(vec![
                XaiEvent::TextDelta { text: "shared reply".to_string() },
                XaiEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("shared")]),
                "r.shared",
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
            let fval: serde_json::Value =
                serde_json::from_slice(payloads.last().unwrap()).unwrap();
            let fork_id = fval["sessionId"].as_str().unwrap().to_string();

            // Parent-only prompt — verify fork's message does NOT appear.
            h.http.push(vec![XaiEvent::Done]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("parent-only")]),
                "r.parent",
            );
            h.expect_n_publishes(4).await;
            let parent_call = h.http.last_call().unwrap();
            let parent_contents: Vec<_> =
                parent_call.inputs.iter().filter_map(|i| i.content()).collect();
            assert!(
                parent_contents.contains(&"parent-only"),
                "parent inputs must contain 'parent-only'; got: {parent_contents:?}"
            );
            assert!(
                !parent_contents.contains(&"fork-only"),
                "parent inputs must NOT contain 'fork-only'; got: {parent_contents:?}"
            );

            // Fork-only prompt — verify parent's message does NOT appear.
            h.http.push(vec![XaiEvent::Done]);
            let fork_prompt_subj = format!("acp.session.{fork_id}.agent.prompt");
            h.session_req(
                &fork_prompt_subj,
                PromptRequest::new(fork_id.clone(), vec![ContentBlock::from("fork-only")]),
                "r.fork-prompt",
            );
            h.expect_n_publishes(5).await;
            let fork_call = h.http.last_call().unwrap();
            let fork_contents: Vec<_> =
                fork_call.inputs.iter().filter_map(|i| i.content()).collect();
            assert!(
                fork_contents.contains(&"fork-only"),
                "fork inputs must contain 'fork-only'; got: {fork_contents:?}"
            );
            assert!(
                !fork_contents.contains(&"parent-only"),
                "fork inputs must NOT contain 'parent-only'; got: {fork_contents:?}"
            );
        })
        .await;
}

// ── x_search tool enabled ─────────────────────────────────────────────────────

/// Enabling `x_search` on a session must cause the tool to appear in the tools
/// array forwarded to the xAI HTTP client on the next prompt.
#[tokio::test]
async fn x_search_tool_enabled_is_included_in_tools_array_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let set_cfg_subj = format!("acp.session.{sid}.agent.set_config_option");
            h.session_req(
                &set_cfg_subj,
                SetSessionConfigOptionRequest::new(sid.clone(), "x_search", "on"),
                "r.cfg",
            );
            h.expect_n_publishes(2).await;

            h.http.push(vec![XaiEvent::Done]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("search X")]),
                "r.prompt",
            );
            h.expect_n_publishes(3).await;

            let call = h.http.last_call().unwrap();
            assert!(
                call.tools.iter().any(|t| t.name() == "x_search"),
                "x_search must be forwarded to the HTTP client after being enabled; tools: {:?}",
                call.tools.iter().map(|t| t.name()).collect::<Vec<_>>()
            );
        })
        .await;
}

// ── initialize: EnvVar auth method fields ─────────────────────────────────────

/// `initialize` must advertise an `EnvVar` auth method whose first var has
/// `name == "XAI_API_KEY"` and whose `link` points to the xAI API docs.
#[tokio::test]
async fn initialize_env_var_method_advertises_correct_var_name_and_link_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            use agent_client_protocol::AuthMethod;

            let h = Harness::new();
            h.global(
                "acp.agent.initialize",
                InitializeRequest::new(ProtocolVersion::LATEST),
                "r.init",
            );
            let payloads = h.expect_n_publishes(1).await;
            let resp: InitializeResponse = serde_json::from_slice(&payloads[0]).unwrap();

            let ev = resp
                .auth_methods
                .iter()
                .find_map(|m| match m {
                    AuthMethod::EnvVar(e) => Some(e),
                    _ => None,
                })
                .expect("initialize must advertise an EnvVar auth method");

            assert_eq!(ev.vars.len(), 1, "expected exactly one env var entry");
            assert_eq!(
                ev.vars[0].name, "XAI_API_KEY",
                "env var name must be XAI_API_KEY; got: {}",
                ev.vars[0].name
            );
            assert_eq!(
                ev.link.as_deref(),
                Some("https://x.ai/api"),
                "EnvVar link must point to https://x.ai/api; got: {:?}",
                ev.link
            );
        })
        .await;
}

// ── pending tool calls cleared on incomplete response ─────────────────────────

/// When an `Incomplete` response contains a `FunctionCall` event, the pending
/// tool call must be discarded before the continuation request. The continuation
/// HTTP call must NOT include any `FunctionCallOutput` items in its inputs.
#[tokio::test]
async fn pending_tool_calls_cleared_when_incomplete_response_contains_function_call_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // First response: FunctionCall followed by Incomplete(max_turns).
            h.http.push(vec![
                XaiEvent::ResponseId {
                    id: "resp-part".to_string(),
                },
                XaiEvent::FunctionCall {
                    call_id: "cid_discard".to_string(),
                    name: "bash".to_string(),
                    arguments: r#"{"command":"echo hi"}"#.to_string(),
                },
                XaiEvent::Finished {
                    reason: FinishReason::Incomplete,
                    incomplete_reason: Some("max_turns".to_string()),
                },
            ]);
            // Continuation response: finishes normally.
            h.http.push(vec![XaiEvent::Done]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("run bash")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;

            let calls = h.http.calls.lock().unwrap().clone();
            assert_eq!(calls.len(), 2, "expected 2 HTTP calls (initial + continuation)");

            let has_fco = calls[1]
                .inputs
                .iter()
                .any(|i| matches!(i, InputItem::FunctionCallOutput { .. }));
            assert!(
                !has_fco,
                "continuation round must NOT forward a FunctionCallOutput for the discarded tool call; inputs: {:?}",
                calls[1].inputs
            );
        })
        .await;
}

// ── system prompt absent when env var not set ─────────────────────────────────

/// When `XAI_SYSTEM_PROMPT` is not set the first-turn HTTP call must contain
/// exactly one input item — the user message — with no preceding system item.
#[tokio::test]
async fn system_prompt_absent_when_env_var_not_set_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Ensure XAI_SYSTEM_PROMPT is absent during agent construction.
            let h = {
                let _env_guard = env_mutex().lock().unwrap();
                // SAFETY: guarded by ENV_MUTEX; only read during with_deps().
                unsafe { std::env::remove_var("XAI_SYSTEM_PROMPT") };
                Harness::new()
            };

            let sid = create_session(&h).await;
            h.http.push(vec![XaiEvent::Done]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hello")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;

            let call = h.http.last_call().unwrap();
            assert_eq!(
                call.input_len, 1,
                "without XAI_SYSTEM_PROMPT the input must contain only the user message; got {}",
                call.input_len
            );
            assert_eq!(
                call.inputs[0].role(),
                Some("user"),
                "the single input item must have role 'user'"
            );
        })
        .await;
}

// ── trogon-tools integration via NATS ─────────────────────────────────────────

/// A new session must have all trogon-tools in the HTTP request tools list
/// without any explicit set_session_config_option call.
#[tokio::test]
async fn new_session_has_trogon_tools_in_http_request_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![XaiEvent::Done]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("read a file")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;

            let call = h.http.last_call().unwrap();
            let tool_names: Vec<_> = call.tools.iter().map(|t| t.name()).collect();
            for expected in &["read_file", "write_file", "str_replace", "search_files", "git_status"] {
                assert!(
                    tool_names.contains(expected),
                    "{expected} must be in HTTP request tools by default; got: {tool_names:?}"
                );
            }
        })
        .await;
}

/// When the model returns a FunctionCall for a trogon-tool (read_file), the
/// agent must dispatch it and send a follow-up HTTP request with the tool
/// output as a FunctionCallOutput item.
#[tokio::test]
async fn trogon_tool_dispatch_via_nats_sends_follow_up() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Round 1: model requests read_file (path may not exist — we only
            // verify that dispatch happens and output is returned).
            h.http.push(vec![
                XaiEvent::ResponseId { id: "r-nats-1".to_string() },
                XaiEvent::FunctionCall {
                    call_id: "cid-rf-nats".to_string(),
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"/nonexistent_integration_test_file.txt"}"#.to_string(),
                },
                XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
                XaiEvent::Done,
            ]);
            // Round 2: model replies after receiving tool output.
            h.http.push(vec![
                XaiEvent::TextDelta { text: "done".to_string() },
                XaiEvent::Done,
            ]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("read that file")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;

            let calls = h.http.calls.lock().unwrap();
            assert_eq!(calls.len(), 2, "agent must make a follow-up call after trogon-tool dispatch");
            let follow_up_inputs = &calls[1].inputs;
            assert!(
                follow_up_inputs.iter().any(|item| matches!(
                    item, InputItem::FunctionCallOutput { call_id, .. } if call_id == "cid-rf-nats"
                )),
                "follow-up must contain FunctionCallOutput for cid-rf-nats"
            );
        })
        .await;
}

/// read_file with a relative path must resolve against the session's cwd.
/// Creates a real file in a tempdir, opens a session with that dir as cwd,
/// then asks the model to read the file by relative name — the follow-up must
/// contain the file content.
#[tokio::test]
async fn trogon_tool_uses_session_cwd_for_relative_path_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            use tempfile::TempDir;
            let dir = TempDir::new().unwrap();
            std::fs::write(dir.path().join("marker.txt"), "nats-cwd-content\n").unwrap();
            let cwd = dir.path().to_str().unwrap().to_string();

            let h = Harness::new();

            // Create a session with the tempdir as cwd.
            let before = h.nats.published_payloads().len();
            h.global(
                "acp.agent.session.new",
                NewSessionRequest::new(&cwd),
                "r.new_cwd",
            );
            let payloads = h.expect_n_publishes(before + 1).await;
            let val: serde_json::Value =
                serde_json::from_slice(payloads.last().unwrap()).unwrap();
            let sid = val["sessionId"].as_str().unwrap().to_string();

            // Round 1: model requests read_file with a relative path.
            h.http.push(vec![
                XaiEvent::ResponseId { id: "r-cwd-nats".to_string() },
                XaiEvent::FunctionCall {
                    call_id: "cid-cwd-nats".to_string(),
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"marker.txt"}"#.to_string(),
                },
                XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
                XaiEvent::Done,
            ]);
            // Round 2: model replies after tool output.
            h.http.push(vec![
                XaiEvent::TextDelta { text: "done".to_string() },
                XaiEvent::Done,
            ]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("read the marker")]),
                "r.prompt_cwd",
            );
            h.expect_n_publishes(before + 2).await;

            let calls = h.http.calls.lock().unwrap();
            assert_eq!(calls.len(), 2, "agent must make a follow-up after read_file");
            let output = calls[1].inputs.iter().find_map(|item| {
                if let InputItem::FunctionCallOutput { call_id, output, .. } = item {
                    if call_id == "cid-cwd-nats" { Some(output.as_str()) } else { None }
                } else {
                    None
                }
            });
            assert!(output.is_some(), "follow-up must contain FunctionCallOutput for cid-cwd-nats");
            assert!(
                output.unwrap().contains("nats-cwd-content"),
                "tool output must contain file content resolved from session cwd, got: {:?}",
                output
            );
        })
        .await;
}

/// fetch_url targeting a link-local / metadata address must be blocked by the
/// egress policy.  The agent must still send a follow-up with the error string.
#[tokio::test]
async fn fetch_url_egress_blocked_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                XaiEvent::ResponseId { id: "r-egress-nats".to_string() },
                XaiEvent::FunctionCall {
                    call_id: "cid-fetch-nats".to_string(),
                    name: "fetch_url".to_string(),
                    arguments: r#"{"url":"http://169.254.169.254/latest/meta-data/"}"#.to_string(),
                },
                XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
                XaiEvent::Done,
            ]);
            h.http.push(vec![
                XaiEvent::TextDelta { text: "blocked".to_string() },
                XaiEvent::Done,
            ]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("fetch metadata")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;

            let calls = h.http.calls.lock().unwrap();
            assert_eq!(calls.len(), 2, "agent must make a follow-up after blocked fetch_url");
            let output = calls[1].inputs.iter().find_map(|item| {
                if let InputItem::FunctionCallOutput { call_id, output, .. } = item {
                    if call_id == "cid-fetch-nats" { Some(output.as_str()) } else { None }
                } else {
                    None
                }
            });
            assert!(output.is_some(), "follow-up must contain FunctionCallOutput for cid-fetch-nats");
            assert!(
                output.unwrap().contains("blocked by egress policy"),
                "output must contain egress block message, got: {:?}", output
            );
        })
        .await;
}
