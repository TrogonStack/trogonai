//! End-to-end integration tests for `trogon-openrouter-runner`.
//!
//! Wires `OpenRouterAgent` through `AgentSideNatsConnection` using `MockNatsClient`
//! as the NATS transport and inline stubs for `OpenRouterHttpClient` and
//! `SessionNotifier`. No real NATS or Docker required.
//!
//! Run with:
//!   cargo test -p trogon-openrouter-runner --test e2e_mock

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use acp_nats::AcpPrefix;
use acp_nats_agent::AgentSideNatsConnection;
use agent_client_protocol::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, CloseSessionRequest,
    CloseSessionResponse, ContentBlock, ForkSessionRequest, InitializeRequest, InitializeResponse,
    ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, ProtocolVersion,
    ResumeSessionRequest, ResumeSessionResponse, SessionNotification,
    SetSessionModeRequest, SetSessionModeResponse, SetSessionModelRequest, SetSessionModelResponse,
};
use async_nats::Message;
use async_trait::async_trait;
use futures::channel::mpsc::UnboundedSender;
use futures_util::StreamExt as _;
use futures_util::stream::{self, LocalBoxStream};
use trogon_nats::mocks::MockNatsClient;
use trogon_openrouter_runner::{
    Message as OaMessage, OpenRouterAgent, OpenRouterEvent, OpenRouterHttpClient, SessionNotifier,
};

// ── Inline HTTP client stub ───────────────────────────────────────────────────

enum TestResponse {
    Events(Vec<OpenRouterEvent>),
    Slow(OpenRouterEvent),
}

#[derive(Clone)]
struct TestHttpClient {
    queue: Arc<Mutex<VecDeque<TestResponse>>>,
}

impl TestHttpClient {
    fn new() -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    fn push(&self, events: Vec<OpenRouterEvent>) {
        self.queue
            .lock()
            .unwrap()
            .push_back(TestResponse::Events(events));
    }

    fn push_slow(&self, first: OpenRouterEvent) {
        self.queue
            .lock()
            .unwrap()
            .push_back(TestResponse::Slow(first));
    }
}

#[async_trait(?Send)]
impl OpenRouterHttpClient for TestHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _messages: &[OaMessage],
        _api_key: &str,
    ) -> LocalBoxStream<'static, OpenRouterEvent> {
        let response = self
            .queue
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(TestResponse::Events(vec![]));
        match response {
            TestResponse::Events(events) => stream::iter(events).boxed_local(),
            TestResponse::Slow(first) => stream::once(async move { first })
                .chain(stream::pending::<OpenRouterEvent>())
                .boxed_local(),
        }
    }
}

// ── Inline session notifier stub ──────────────────────────────────────────────

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
    http: TestHttpClient,
    notifier: TestNotifier,
    global_tx: UnboundedSender<Message>,
    session_tx: UnboundedSender<Message>,
}

impl Harness {
    fn new() -> Self {
        Self::with_api_key("test-key")
    }

    fn with_api_key(key: &str) -> Self {
        let nats = MockNatsClient::new();
        let http = TestHttpClient::new();
        let notifier = TestNotifier::new();

        let global_tx = nats.inject_messages();
        let session_tx = nats.inject_messages();

        let http_clone = http.clone();
        let notifier_clone = notifier.clone();

        let prefix = AcpPrefix::new("acp").unwrap();
        let agent = OpenRouterAgent::with_deps(notifier_clone, "test-model", key, http_clone);
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

    fn global(&self, subject: &str, payload: impl serde::Serialize, reply: &str) {
        self.inject(&self.global_tx, subject, payload, Some(reply));
    }

    fn session_req(&self, subject: &str, payload: impl serde::Serialize, reply: &str) {
        self.inject(&self.session_tx, subject, payload, Some(reply));
    }

    fn global_notify(&self, subject: &str, payload: impl serde::Serialize) {
        self.inject(&self.global_tx, subject, payload, None);
    }

    fn session_notify(&self, subject: &str, payload: impl serde::Serialize) {
        self.inject(&self.session_tx, subject, payload, None);
    }

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

    fn published_subjects(&self) -> Vec<String> {
        self.nats.published_messages()
    }
}

// ── Shared helpers ────────────────────────────────────────────────────────────

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

// ── initialize ────────────────────────────────────────────────────────────────

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
            let caps = &resp.agent_capabilities;
            assert!(caps.load_session, "must advertise load_session");
        })
        .await;
}

// ── new_session ───────────────────────────────────────────────────────────────

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
            let resp: NewSessionResponse = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(!resp.session_id.to_string().is_empty(), "session_id must not be empty");
            assert!(resp.modes.is_some(), "must return mode state");
            assert!(resp.models.is_some(), "must return model state");
        })
        .await;
}

// ── prompt ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn prompt_via_nats_returns_prompt_response() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                OpenRouterEvent::TextDelta { text: "hello".to_string() },
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

#[tokio::test]
async fn prompt_via_nats_sends_text_notifications() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                OpenRouterEvent::TextDelta { text: "chunk1".to_string() },
                OpenRouterEvent::TextDelta { text: "chunk2".to_string() },
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;
            h.expect_n_notifications(2).await;
            assert_eq!(h.notifier.count(), 2, "one notification per TextDelta");
        })
        .await;
}

#[tokio::test]
async fn prompt_via_nats_sends_usage_notification() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                OpenRouterEvent::TextDelta { text: "answer".to_string() },
                OpenRouterEvent::Usage { prompt_tokens: 10, completion_tokens: 5 },
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.prompt",
            );
            // TextDelta + Usage = 2 notifications
            h.expect_n_publishes(2).await;
            h.expect_n_notifications(2).await;
        })
        .await;
}

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

#[tokio::test]
async fn authenticate_user_key_via_nats_succeeds() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let mut meta = serde_json::Map::new();
            meta.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!("sk-test-123"));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("openrouter-api-key").meta(meta),
                "r.auth",
            );
            let payloads = h.expect_n_publishes(1).await;
            let _: AuthenticateResponse = serde_json::from_slice(&payloads[0]).unwrap();
        })
        .await;
}

#[tokio::test]
async fn authenticate_then_new_session_uses_pending_key() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Agent with no global key — prompt without authenticate must fail.
            let h = Harness::with_api_key("");

            // Authenticate to store the pending key.
            let mut meta = serde_json::Map::new();
            meta.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!("sk-user-key"));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("openrouter-api-key").meta(meta),
                "r.auth",
            );
            h.expect_n_publishes(1).await;

            // Create session — must consume the pending key.
            let sid = create_session(&h).await;

            // Prompt must succeed (key is now attached to the session).
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );
            let payloads = h.expect_n_publishes(3).await; // auth + session/new + prompt
            let val: serde_json::Value = serde_json::from_slice(&payloads[2]).unwrap();
            assert!(
                val.get("code").is_none(),
                "prompt must succeed after authenticate, got: {val}"
            );
        })
        .await;
}

#[tokio::test]
async fn authenticate_agent_method_succeeds_with_global_key() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Agent has a global key, so "agent" method is available.
            let h = Harness::with_api_key("global-key");
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("agent"),
                "r.auth",
            );
            let payloads = h.expect_n_publishes(1).await;
            let _: AuthenticateResponse = serde_json::from_slice(&payloads[0]).unwrap();
        })
        .await;
}

// ── close_session ─────────────────────────────────────────────────────────────

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

// ── resume_session ────────────────────────────────────────────────────────────

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

#[tokio::test]
async fn resume_unknown_session_returns_acp_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.session_req(
                "acp.session.ghost.agent.resume",
                ResumeSessionRequest::new("ghost", "/"),
                "r.err",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(val.get("code").is_some(), "must return ACP error: {val}");
        })
        .await;
}

// ── load_session ──────────────────────────────────────────────────────────────

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
            let resp: LoadSessionResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(resp.modes.is_some(), "load must return mode state");
            assert!(resp.models.is_some(), "load must return model state");
        })
        .await;
}

#[tokio::test]
async fn load_unknown_session_returns_acp_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.session_req(
                "acp.session.ghost.agent.load",
                LoadSessionRequest::new("ghost", "/"),
                "r.err",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(val.get("code").is_some(), "must return ACP error: {val}");
        })
        .await;
}

// ── fork_session ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn fork_session_via_nats_creates_new_session() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let fork_subj = format!("acp.session.{sid}.agent.fork");
            h.session_req(
                &fork_subj,
                ForkSessionRequest::new(sid.clone(), "/fork"),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(2).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[1]).unwrap();
            let fork_id = val["sessionId"].as_str().unwrap_or_default();
            assert!(!fork_id.is_empty(), "fork must return non-empty sessionId");
            assert_ne!(fork_id, sid.as_str(), "fork sessionId must differ from source");
        })
        .await;
}

#[tokio::test]
async fn fork_session_and_prompt_independently() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Prompt source session.
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "from src".to_string() }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q1")]),
                "r.p1",
            );
            h.expect_n_publishes(2).await;

            // Fork.
            let fork_subj = format!("acp.session.{sid}.agent.fork");
            h.session_req(
                &fork_subj,
                ForkSessionRequest::new(sid.clone(), "/fork"),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(3).await;
            let fork_id = serde_json::from_slice::<serde_json::Value>(&payloads[2]).unwrap()
                ["sessionId"].as_str().unwrap().to_string();

            // Prompt the fork independently.
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "from fork".to_string() }]);
            let fork_prompt_subj = format!("acp.session.{fork_id}.agent.prompt");
            h.session_req(
                &fork_prompt_subj,
                PromptRequest::new(fork_id.clone(), vec![ContentBlock::from("q2")]),
                "r.p2",
            );
            let payloads = h.expect_n_publishes(4).await;
            let _: PromptResponse = serde_json::from_slice(&payloads[3]).unwrap();
        })
        .await;
}

// ── list_sessions ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_via_nats_returns_sorted() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            create_session(&h).await;
            create_session(&h).await;

            h.global("acp.agent.session.list", ListSessionsRequest::new(), "r.list");
            let payloads = h.expect_n_publishes(3).await;
            let resp: ListSessionsResponse = serde_json::from_slice(&payloads[2]).unwrap();
            assert_eq!(resp.sessions.len(), 2, "must list both sessions");

            let ids: Vec<String> = resp.sessions.iter().map(|s| s.session_id.to_string()).collect();
            let mut sorted = ids.clone();
            sorted.sort();
            assert_eq!(ids, sorted, "sessions must be sorted by ID: {ids:?}");
        })
        .await;
}

// ── set_session_model ─────────────────────────────────────────────────────────

#[tokio::test]
async fn set_session_model_via_nats_updates_model() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // "test-model" is added automatically since it's the default.
            let set_model_subj = format!("acp.session.{sid}.agent.set_model");
            h.session_req(
                &set_model_subj,
                SetSessionModelRequest::new(sid.clone(), "test-model"),
                "r.set_model",
            );
            let payloads = h.expect_n_publishes(2).await;
            let _: SetSessionModelResponse = serde_json::from_slice(&payloads[1]).unwrap();

            // Verify via load_session that the change persisted.
            let load_subj = format!("acp.session.{sid}.agent.load");
            h.session_req(
                &load_subj,
                LoadSessionRequest::new(sid.clone(), "/tmp"),
                "r.load",
            );
            let payloads = h.expect_n_publishes(3).await;
            let resp: LoadSessionResponse = serde_json::from_slice(&payloads[2]).unwrap();
            let current = resp.models.unwrap().current_model_id.to_string();
            assert_eq!(current, "test-model", "model must persist after set_model");
        })
        .await;
}

#[tokio::test]
async fn set_session_model_unknown_model_returns_acp_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let set_model_subj = format!("acp.session.{sid}.agent.set_model");
            h.session_req(
                &set_model_subj,
                SetSessionModelRequest::new(sid.clone(), "no-such-model"),
                "r.err",
            );
            let payloads = h.expect_n_publishes(2).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(val.get("code").is_some(), "unknown model must return ACP error: {val}");
        })
        .await;
}

// ── set_session_mode ──────────────────────────────────────────────────────────

#[tokio::test]
async fn set_session_mode_default_succeeds() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let set_mode_subj = format!("acp.session.{sid}.agent.set_mode");
            h.session_req(
                &set_mode_subj,
                SetSessionModeRequest::new(sid.clone(), "default"),
                "r.set_mode",
            );
            let payloads = h.expect_n_publishes(2).await;
            let _: SetSessionModeResponse = serde_json::from_slice(&payloads[1]).unwrap();
        })
        .await;
}

#[tokio::test]
async fn set_session_mode_unknown_returns_acp_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let set_mode_subj = format!("acp.session.{sid}.agent.set_mode");
            h.session_req(
                &set_mode_subj,
                SetSessionModeRequest::new(sid.clone(), "turbo"),
                "r.err",
            );
            let payloads = h.expect_n_publishes(2).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(val.get("code").is_some(), "unknown mode must return ACP error: {val}");
        })
        .await;
}

// ── cancel ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_via_nats_interrupts_prompt() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push_slow(OpenRouterEvent::TextDelta { text: "partial".to_string() });

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

            let cancel_subj = format!("acp.session.{sid}.agent.cancel");
            h.session_notify(&cancel_subj, CancelNotification::new(sid.clone()));

            // Prompt must complete and publish a response.
            let payloads = h.expect_n_publishes(2).await;
            let _: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
        })
        .await;
}

#[tokio::test]
async fn cancel_noop_for_unknown_session_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.session_notify(
                "acp.session.ghost.agent.cancel",
                CancelNotification::new("ghost"),
            );
            h.expect_no_new_publishes(0).await;
        })
        .await;
}

// ── reply subject routing / edge cases ───────────────────────────────────────

#[tokio::test]
async fn reply_subject_routing_is_correct() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.global(
                "acp.agent.initialize",
                InitializeRequest::new(ProtocolVersion::LATEST),
                "reply.unique.99999",
            );
            h.expect_n_publishes(1).await;
            let subjects = h.published_subjects();
            assert!(
                subjects.contains(&"reply.unique.99999".to_string()),
                "response must be published to exact reply subject; got: {subjects:?}"
            );
        })
        .await;
}

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
                "malformed request must produce ACP error: {val}"
            );
        })
        .await;
}

#[tokio::test]
async fn request_without_reply_subject_does_not_publish() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.global_notify(
                "acp.agent.initialize",
                InitializeRequest::new(ProtocolVersion::LATEST),
            );
            h.expect_no_new_publishes(0).await;
        })
        .await;
}
