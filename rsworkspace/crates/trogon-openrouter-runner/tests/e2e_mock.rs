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
    AuthMethod, AuthenticateRequest, AuthenticateResponse, BlobResourceContents, CancelNotification,
    CloseSessionRequest, CloseSessionResponse, ContentBlock, EmbeddedResource,
    EmbeddedResourceResource, ForkSessionRequest, ImageContent, InitializeRequest,
    InitializeResponse, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    ProtocolVersion, ResourceLink, ResumeSessionRequest, ResumeSessionResponse, SessionConfigKind,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse,
    SetSessionModelRequest, SetSessionModelResponse, TextResourceContents,
    ToolCallStatus, ToolKind,
};
use async_nats::Message;
use async_trait::async_trait;
use futures::channel::mpsc::UnboundedSender;
use futures_util::StreamExt as _;
use futures_util::stream::{self, LocalBoxStream};
use trogon_nats::mocks::MockNatsClient;
use trogon_openrouter_runner::{
    AgentConfig, AgentLoading, AssembledToolCall, FinishReason, Message as OaMessage,
    OpenRouterAgent, OpenRouterEvent, OpenRouterHttpClient, SessionNotifier, SkillLoading, ToolDef,
};

// ── Inline HTTP client stub ───────────────────────────────────────────────────

enum TestResponse {
    Events(Vec<OpenRouterEvent>),
    Slow(OpenRouterEvent),
}

#[derive(Clone)]
struct TestHttpClient {
    queue: Arc<Mutex<VecDeque<TestResponse>>>,
    /// Records the tool names passed in each chat_stream call.
    recorded_tool_names: Arc<Mutex<Vec<Vec<String>>>>,
    /// Records the model ID passed in each chat_stream call.
    recorded_models: Arc<Mutex<Vec<String>>>,
    /// Records the API key passed in each chat_stream call.
    recorded_api_keys: Arc<Mutex<Vec<String>>>,
    /// Records the full messages slice passed in each chat_stream call.
    recorded_messages: Arc<Mutex<Vec<Vec<OaMessage>>>>,
}

impl TestHttpClient {
    fn new() -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            recorded_tool_names: Arc::new(Mutex::new(Vec::new())),
            recorded_models: Arc::new(Mutex::new(Vec::new())),
            recorded_api_keys: Arc::new(Mutex::new(Vec::new())),
            recorded_messages: Arc::new(Mutex::new(Vec::new())),
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

    fn last_tool_names(&self) -> Vec<String> {
        self.recorded_tool_names.lock().unwrap().last().cloned().unwrap_or_default()
    }

    fn last_model(&self) -> String {
        self.recorded_models.lock().unwrap().last().cloned().unwrap_or_default()
    }

    fn last_api_key(&self) -> String {
        self.recorded_api_keys.lock().unwrap().last().cloned().unwrap_or_default()
    }

    fn last_messages(&self) -> Vec<OaMessage> {
        self.recorded_messages.lock().unwrap().last().cloned().unwrap_or_default()
    }
}

#[async_trait(?Send)]
impl OpenRouterHttpClient for TestHttpClient {
    async fn chat_stream(
        &self,
        model: &str,
        messages: &[OaMessage],
        api_key: &str,
        tools: &[ToolDef],
    ) -> LocalBoxStream<'static, OpenRouterEvent> {
        self.recorded_models.lock().unwrap().push(model.to_string());
        self.recorded_api_keys.lock().unwrap().push(api_key.to_string());
        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        self.recorded_tool_names.lock().unwrap().push(names);
        self.recorded_messages.lock().unwrap().push(messages.to_vec());
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

fn inject_req(
    tx: &futures::channel::mpsc::UnboundedSender<async_nats::Message>,
    subject: &str,
    payload: impl serde::Serialize,
    reply: &str,
) {
    let raw: bytes::Bytes = serde_json::to_vec(&payload).unwrap().into();
    let msg = async_nats::Message {
        subject: subject.into(),
        reply: Some(reply.into()),
        payload: raw,
        headers: None,
        length: 0,
        status: None,
        description: None,
    };
    tx.unbounded_send(msg).unwrap();
}

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
                OpenRouterEvent::Usage { prompt_tokens: 10, completion_tokens: 5, cache_read_tokens: 0, cache_creation_tokens: 0 },
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

#[tokio::test]
async fn close_session_then_list_sessions_removes_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Close the session.
            let close_subj = format!("acp.session.{sid}.agent.close");
            h.session_req(
                &close_subj,
                CloseSessionRequest::new(sid.clone()),
                "r.close",
            );
            h.expect_n_publishes(2).await;

            // List sessions — closed one must not appear.
            h.global("acp.agent.session.list", ListSessionsRequest::new(), "r.list");
            let payloads = h.expect_n_publishes(3).await;
            let resp: ListSessionsResponse = serde_json::from_slice(&payloads[2]).unwrap();
            assert!(
                resp.sessions.is_empty(),
                "closed session must not appear in list: {:?}",
                resp.sessions
            );
        })
        .await;
}

#[tokio::test]
async fn fork_session_with_branch_at_index_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Fork with branchAtIndex.
            let fork_subj = format!("acp.session.{sid}.agent.fork");
            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({"branchAtIndex": 0}),
            )
            .unwrap();
            h.session_req(
                &fork_subj,
                ForkSessionRequest::new(sid.clone(), "/fork").meta(meta),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(2).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[1]).unwrap();
            let fork_id = val["sessionId"].as_str().unwrap_or_default();
            assert!(!fork_id.is_empty(), "fork must return non-empty sessionId");
            assert_ne!(fork_id, sid.as_str(), "fork sessionId must differ from source");
            // The fork response includes modes and models.
            assert!(val.get("modes").is_some(), "fork response must include modes");
            assert!(val.get("models").is_some(), "fork response must include models");
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

            // Wait for the TextDelta notification — guarantees cancel_senders is registered
            // and the slow stream is still pending.
            h.expect_n_notifications(1).await;

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

// ── config_options in session responses ──────────────────────────────────────

#[tokio::test]
async fn new_session_via_nats_returns_config_options() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.global(
                "acp.agent.session.new",
                NewSessionRequest::new("/workspace"),
                "reply.new.cfg",
            );
            let payloads = h.expect_n_publishes(1).await;
            let resp: NewSessionResponse = serde_json::from_slice(&payloads[0]).unwrap();
            let config_options = resp.config_options.unwrap_or_default();
            assert!(!config_options.is_empty(), "new_session response must include config_options");
            assert!(
                config_options.iter().any(|o| o.id.to_string() == "read_file"),
                "config_options must include read_file tool"
            );
        })
        .await;
}

#[tokio::test]
async fn load_session_via_nats_returns_config_options() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let subj = format!("acp.session.{sid}.agent.load");
            h.session_req(&subj, LoadSessionRequest::new(sid.clone(), "/"), "reply.load.cfg");
            let payloads = h.expect_n_publishes(2).await;
            let resp: LoadSessionResponse = serde_json::from_slice(&payloads[1]).unwrap();
            let config_options = resp.config_options.unwrap_or_default();
            assert!(!config_options.is_empty(), "load_session response must include config_options");
        })
        .await;
}

#[tokio::test]
async fn fork_session_via_nats_returns_config_options() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let subj = format!("acp.session.{sid}.agent.fork");
            h.session_req(&subj, ForkSessionRequest::new(sid.clone(), "/fork"), "reply.fork.cfg");
            let payloads = h.expect_n_publishes(2).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                val.get("configOptions").is_some(),
                "fork response must include configOptions: {val}"
            );
        })
        .await;
}

// ── set_session_config_option via NATS ────────────────────────────────────────

#[tokio::test]
async fn set_session_config_option_via_nats_disables_tool() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let subj = format!("acp.session.{sid}.agent.set_config_option");
            h.session_req(
                &subj,
                SetSessionConfigOptionRequest::new(sid.clone(), "read_file", "disabled"),
                "reply.set_cfg",
            );
            let payloads = h.expect_n_publishes(2).await;
            let resp: SetSessionConfigOptionResponse =
                serde_json::from_slice(&payloads[1]).unwrap();
            let read_file_opt = resp
                .config_options
                .iter()
                .find(|o| o.id.to_string() == "read_file")
                .expect("read_file must be in response config_options");
            let current = match &read_file_opt.kind {
                agent_client_protocol::SessionConfigKind::Select(s) => s.current_value.to_string(),
                _ => String::new(),
            };
            assert_eq!(current, "disabled", "read_file must show disabled after set_config_option");
        })
        .await;
}

#[tokio::test]
async fn set_session_config_option_via_nats_enables_then_disables() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Disable
            let subj = format!("acp.session.{sid}.agent.set_config_option");
            h.session_req(
                &subj,
                SetSessionConfigOptionRequest::new(sid.clone(), "read_file", "disabled"),
                "r.disable",
            );
            h.expect_n_publishes(2).await;

            // Re-enable
            h.session_req(
                &subj,
                SetSessionConfigOptionRequest::new(sid.clone(), "read_file", "enabled"),
                "r.enable",
            );
            let payloads = h.expect_n_publishes(3).await;
            let resp: SetSessionConfigOptionResponse =
                serde_json::from_slice(&payloads[2]).unwrap();
            let opt = resp
                .config_options
                .iter()
                .find(|o| o.id.to_string() == "read_file")
                .unwrap();
            let current = match &opt.kind {
                agent_client_protocol::SessionConfigKind::Select(s) => s.current_value.to_string(),
                _ => String::new(),
            };
            assert_eq!(current, "enabled", "read_file must be re-enabled");
        })
        .await;
}

// ── tool call round-trip via NATS ─────────────────────────────────────────────

#[tokio::test]
async fn tool_call_round_trip_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // First HTTP response: tool call
            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_1".to_string(),
                    name: "list_directory".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                }],
            }]);
            // Second HTTP response: final answer after tool result
            h.http.push(vec![OpenRouterEvent::TextDelta {
                text: "done".to_string(),
            }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("list files")]),
                "r.tool_trip",
            );

            // 1 publish for new_session + 1 for prompt response
            let payloads = h.expect_n_publishes(2).await;
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "tool round-trip must resolve to EndTurn: {:?}",
                resp.stop_reason
            );
        })
        .await;
}

#[tokio::test]
async fn tool_call_notifications_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_n".to_string(),
                    name: "list_directory".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                }],
            }]);
            h.http.push(vec![OpenRouterEvent::TextDelta {
                text: "done".to_string(),
            }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.notifs",
            );
            h.expect_n_publishes(2).await;

            let notes = h.notifier.notifications.lock().unwrap();
            let has_tool_call = notes
                .iter()
                .any(|n| matches!(&n.update, SessionUpdate::ToolCall(_)));
            let has_tool_call_update = notes
                .iter()
                .any(|n| matches!(&n.update, SessionUpdate::ToolCallUpdate(_)));
            assert!(has_tool_call, "must emit ToolCall (InProgress) notification over NATS");
            assert!(
                has_tool_call_update,
                "must emit ToolCallUpdate (Completed) notification over NATS"
            );
        })
        .await;
}

// ── resume_session returns config_options ─────────────────────────────────────

#[tokio::test]
async fn resume_session_via_nats_returns_config_options() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let subj = format!("acp.session.{sid}.agent.resume");
            h.session_req(&subj, ResumeSessionRequest::new(sid.clone(), "/"), "r.resume.cfg");
            let payloads = h.expect_n_publishes(2).await;
            let resp: ResumeSessionResponse = serde_json::from_slice(&payloads[1]).unwrap();
            let config_options = resp.config_options.unwrap_or_default();
            assert!(
                !config_options.is_empty(),
                "resume_session response must include config_options"
            );
            assert!(
                config_options.iter().any(|o| o.id.to_string() == "read_file"),
                "config_options must include read_file tool"
            );
        })
        .await;
}

// ── disabled tool not sent after set_config_option ────────────────────────────

#[tokio::test]
async fn disabled_tool_not_sent_in_prompt_after_set_config_option() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Disable read_file
            let cfg_subj = format!("acp.session.{sid}.agent.set_config_option");
            h.session_req(
                &cfg_subj,
                SetSessionConfigOptionRequest::new(sid.clone(), "read_file", "disabled"),
                "r.cfg",
            );
            h.expect_n_publishes(2).await;

            // Prompt — agent will call http.chat_stream with the updated tool list
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.prompt.cfg",
            );
            h.expect_n_publishes(3).await;

            let tool_names = h.http.last_tool_names();
            assert!(
                !tool_names.iter().any(|n| n == "read_file"),
                "disabled read_file must not be sent in prompt tools: {tool_names:?}"
            );
        })
        .await;
}

// ── fork inherits disabled tools via NATS ─────────────────────────────────────

#[tokio::test]
async fn fork_session_inherits_disabled_tools_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Disable read_file in source
            let cfg_subj = format!("acp.session.{sid}.agent.set_config_option");
            h.session_req(
                &cfg_subj,
                SetSessionConfigOptionRequest::new(sid.clone(), "read_file", "disabled"),
                "r.cfg.fork",
            );
            h.expect_n_publishes(2).await;

            // Fork — configOptions in response must reflect the inherited state
            let fork_subj = format!("acp.session.{sid}.agent.fork");
            h.session_req(
                &fork_subj,
                ForkSessionRequest::new(sid.clone(), "/fork"),
                "r.fork.inherit",
            );
            let payloads = h.expect_n_publishes(3).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[2]).unwrap();
            let config_options = val["configOptions"].as_array()
                .expect("fork response must have configOptions");
            let read_file = config_options
                .iter()
                .find(|o| o["id"].as_str() == Some("read_file"))
                .expect("read_file must appear in fork configOptions");
            assert_eq!(
                read_file["currentValue"].as_str(),
                Some("disabled"),
                "fork must inherit disabled read_file from source: {read_file}"
            );
        })
        .await;
}

// ── max tool rounds ───────────────────────────────────────────────────────────

#[tokio::test]
async fn max_tool_rounds_stops_after_10_cycles() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // 11 ToolCallsReady: rounds 1-10 execute normally; on round 11 the
            // MAX_TOOL_ROUNDS=10 guard fires before dispatching and returns Cancelled.
            for _ in 0..11 {
                h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                    calls: vec![AssembledToolCall {
                        id: "call_x".to_string(),
                        name: "list_directory".to_string(),
                        arguments: r#"{"path":"."}"#.to_string(),
                    }],
                }]);
            }

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("loop")]),
                "r.max_rounds",
            );

            let payloads = h.expect_n_publishes(2).await;
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::Cancelled),
                "must stop with Cancelled after 10 tool rounds: {:?}",
                resp.stop_reason
            );
        })
        .await;
}

// ── list_sessions fork metadata ───────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_via_nats_includes_fork_metadata() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let src_id = create_session(&h).await;

            let fork_subj = format!("acp.session.{src_id}.agent.fork");
            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({ "branchAtIndex": 0 }),
            )
            .unwrap();
            h.session_req(
                &fork_subj,
                ForkSessionRequest::new(src_id.clone(), "/fork").meta(meta),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(2).await;
            let fork_id = serde_json::from_slice::<serde_json::Value>(&payloads[1]).unwrap()
                ["sessionId"]
                .as_str()
                .unwrap()
                .to_string();

            h.global("acp.agent.session.list", ListSessionsRequest::new(), "r.list");
            let payloads = h.expect_n_publishes(3).await;
            let resp: ListSessionsResponse = serde_json::from_slice(&payloads[2]).unwrap();

            let fork_info = resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == fork_id)
                .expect("fork session must appear in list");
            let fork_meta = fork_info.meta.as_ref().expect("fork session must have _meta");
            assert!(
                fork_meta.contains_key("parentSessionId"),
                "fork _meta must contain parentSessionId: {fork_meta:?}"
            );
            assert_eq!(
                fork_meta["parentSessionId"].as_str(),
                Some(src_id.as_str()),
                "parentSessionId must point to source session"
            );
            assert!(
                fork_meta.contains_key("branchedAtIndex"),
                "fork _meta must contain branchedAtIndex: {fork_meta:?}"
            );
            assert_eq!(
                fork_meta["branchedAtIndex"],
                serde_json::json!(0),
                "branchedAtIndex must be 0"
            );
        })
        .await;
}

// ── close session during active prompt ────────────────────────────────────────

#[tokio::test]
async fn close_session_during_active_prompt_cancels_and_removes_session() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Slow HTTP response: yields one event then hangs forever.
            h.http.push_slow(OpenRouterEvent::TextDelta { text: "partial".to_string() });

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );

            // Wait for the TextDelta notification — guarantees cancel_senders is registered
            // and the slow stream is still pending.
            h.expect_n_notifications(1).await;

            // Close the session while the prompt is still running.
            let close_subj = format!("acp.session.{sid}.agent.close");
            h.session_req(
                &close_subj,
                CloseSessionRequest::new(sid.clone()),
                "r.close",
            );

            // Expect new_session + close_session + prompt (3 total).
            let payloads = h.expect_n_publishes(3).await;
            let has_prompt_resp = payloads[1..]
                .iter()
                .any(|p| serde_json::from_slice::<PromptResponse>(p).is_ok());
            assert!(has_prompt_resp, "prompt must complete after close_session");

            // Session must not appear in list_sessions.
            h.global("acp.agent.session.list", ListSessionsRequest::new(), "r.list");
            let payloads = h.expect_n_publishes(4).await;
            let resp: ListSessionsResponse = serde_json::from_slice(&payloads[3]).unwrap();
            assert!(
                resp.sessions.iter().all(|s| s.session_id.to_string() != sid),
                "closed session must not appear in list_sessions after close during active prompt"
            );
        })
        .await;
}

// ── FinishReason → StopReason mapping ────────────────────────────────────────

#[tokio::test]
async fn finish_reason_stop_maps_to_end_turn() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                OpenRouterEvent::TextDelta { text: "answer".to_string() },
                OpenRouterEvent::Finished { reason: FinishReason::Stop },
            ]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.finish_stop",
            );

            let payloads = h.expect_n_publishes(2).await;
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "FinishReason::Stop must map to StopReason::EndTurn: {:?}",
                resp.stop_reason
            );
        })
        .await;
}

#[tokio::test]
async fn finish_reason_length_maps_to_end_turn() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                OpenRouterEvent::TextDelta { text: "truncated".to_string() },
                OpenRouterEvent::Finished { reason: FinishReason::Length },
            ]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.finish_length",
            );

            let payloads = h.expect_n_publishes(2).await;
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "FinishReason::Length must map to StopReason::EndTurn: {:?}",
                resp.stop_reason
            );
        })
        .await;
}

// ── fork / close unknown session ─────────────────────────────────────────────

#[tokio::test]
async fn fork_unknown_session_returns_acp_error() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.session_req(
                "acp.session.nonexistent-id.agent.fork",
                ForkSessionRequest::new("nonexistent-id", "/fork"),
                "r.fork.unknown",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "forking a nonexistent session must return an ACP error with a code: {val}"
            );
        })
        .await;
}

#[tokio::test]
async fn close_unknown_session_is_noop_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.session_req(
                "acp.session.ghost.agent.close",
                CloseSessionRequest::new("ghost"),
                "r.close.unknown",
            );
            let payloads = h.expect_n_publishes(1).await;
            // Must succeed (no error code) — close is idempotent.
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_none(),
                "closing a nonexistent session must succeed (idempotent), not return an error: {val}"
            );
        })
        .await;
}

// ── list_sessions cwd ─────────────────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_via_nats_returns_cwd() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            // Create session with a specific cwd.
            h.global(
                "acp.agent.session.new",
                NewSessionRequest::new("/my/workspace"),
                "r.new",
            );
            let payloads = h.expect_n_publishes(1).await;
            let sid = serde_json::from_slice::<serde_json::Value>(&payloads[0]).unwrap()
                ["sessionId"]
                .as_str()
                .unwrap()
                .to_string();

            h.global("acp.agent.session.list", ListSessionsRequest::new(), "r.list");
            let payloads = h.expect_n_publishes(2).await;
            let resp: ListSessionsResponse = serde_json::from_slice(&payloads[1]).unwrap();
            let info = resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == sid)
                .expect("created session must appear in list");
            assert_eq!(
                info.cwd.to_string_lossy(),
                "/my/workspace",
                "list_sessions must return the cwd from new_session"
            );
        })
        .await;
}

// ── authenticate: agent method without global key ────────────────────────────

#[tokio::test]
async fn authenticate_agent_method_fails_without_global_key() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Agent created with empty global key — "agent" method must fail.
            let h = Harness::with_api_key("");
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("agent"),
                "r.auth.fail",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "\"agent\" auth method must fail when no global key is configured: {val}"
            );
        })
        .await;
}

// ── set_session_model wires new model to next prompt ─────────────────────────

#[tokio::test]
async fn set_session_model_wires_new_model_to_next_prompt() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let set_model_subj = format!("acp.session.{sid}.agent.set_model");
            h.session_req(
                &set_model_subj,
                SetSessionModelRequest::new(sid.clone(), "test-model"),
                "r.set_model.wire",
            );
            h.expect_n_publishes(2).await;

            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.prompt.model",
            );
            h.expect_n_publishes(3).await;

            assert_eq!(
                h.http.last_model(),
                "test-model",
                "prompt must forward the session model to the HTTP client"
            );
        })
        .await;
}

// ── Error event → EndTurn stop reason ────────────────────────────────────────

#[tokio::test]
async fn error_event_returns_end_turn_stop_reason() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::Error {
                message: "upstream error".to_string(),
            }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.error_event",
            );

            let payloads = h.expect_n_publishes(2).await;
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "Error event must resolve to EndTurn stop reason: {:?}",
                resp.stop_reason
            );
        })
        .await;
}

// ── load_session has modes+models; resume_session does not ───────────────────

#[tokio::test]
async fn load_session_has_modes_and_models_resume_does_not() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let load_subj = format!("acp.session.{sid}.agent.load");
            h.session_req(
                &load_subj,
                LoadSessionRequest::new(sid.clone(), "/"),
                "r.load.struct",
            );
            let payloads = h.expect_n_publishes(2).await;
            let load_resp: LoadSessionResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(load_resp.modes.is_some(), "load_session must include modes");
            assert!(load_resp.models.is_some(), "load_session must include models");

            let resume_subj = format!("acp.session.{sid}.agent.resume");
            h.session_req(
                &resume_subj,
                ResumeSessionRequest::new(sid.clone(), "/"),
                "r.resume.struct",
            );
            let payloads = h.expect_n_publishes(3).await;
            let resume_resp: ResumeSessionResponse = serde_json::from_slice(&payloads[2]).unwrap();
            assert!(resume_resp.modes.is_none(), "resume_session must not include modes");
            assert!(resume_resp.models.is_none(), "resume_session must not include models");
        })
        .await;
}

// ── two sessions have independent histories ───────────────────────────────────

#[tokio::test]
async fn two_sessions_have_independent_histories() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid1 = create_session(&h).await;
            let sid2 = create_session(&h).await;

            // Prompt session 1
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "reply-s1".to_string() }]);
            let prompt1_subj = format!("acp.session.{sid1}.agent.prompt");
            h.session_req(
                &prompt1_subj,
                PromptRequest::new(sid1.clone(), vec![ContentBlock::from("hello from s1")]),
                "r.prompt.s1",
            );
            h.expect_n_publishes(3).await;

            // Prompt session 2
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "reply-s2".to_string() }]);
            let prompt2_subj = format!("acp.session.{sid2}.agent.prompt");
            h.session_req(
                &prompt2_subj,
                PromptRequest::new(sid2.clone(), vec![ContentBlock::from("hello from s2")]),
                "r.prompt.s2",
            );
            let payloads = h.expect_n_publishes(4).await;

            // Both prompts must complete with EndTurn
            let r1: PromptResponse = serde_json::from_slice(&payloads[2]).unwrap();
            let r2: PromptResponse = serde_json::from_slice(&payloads[3]).unwrap();
            assert!(
                matches!(r1.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "s1 must complete with EndTurn: {:?}",
                r1.stop_reason
            );
            assert!(
                matches!(r2.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "s2 must complete with EndTurn: {:?}",
                r2.stop_reason
            );

            // Two separate chat_stream calls confirm independent dispatch
            let call_count = h.http.recorded_models.lock().unwrap().len();
            assert_eq!(call_count, 2, "must make one chat_stream call per session prompt");
        })
        .await;
}

// ── user key takes precedence over global key ─────────────────────────────────

#[tokio::test]
async fn prompt_uses_user_key_over_global_key() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Harness::new() sets global key = "test-key".
            let h = Harness::new();

            // Authenticate with a user-specific key.
            let mut meta = serde_json::Map::new();
            meta.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!("user-specific-key"));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("openrouter-api-key").meta(meta),
                "r.auth",
            );
            h.expect_n_publishes(1).await;

            // new_session consumes the pending user key.
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.prompt.key",
            );
            h.expect_n_publishes(3).await;

            assert_eq!(
                h.http.last_api_key(),
                "user-specific-key",
                "user-provided key must take precedence over global server key"
            );
        })
        .await;
}

// ── fork inherits api key and uses it for prompt ──────────────────────────────

#[tokio::test]
async fn fork_inherits_api_key_and_uses_it_for_prompt() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // No global key — all key supply from authenticate.
            let h = Harness::with_api_key("");

            let mut meta = serde_json::Map::new();
            meta.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!("inherited-key"));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("openrouter-api-key").meta(meta),
                "r.auth",
            );
            h.expect_n_publishes(1).await;

            // Source session consumes the pending key.
            let src_id = create_session(&h).await;

            // Fork the source — must inherit the key.
            let fork_subj = format!("acp.session.{src_id}.agent.fork");
            h.session_req(
                &fork_subj,
                ForkSessionRequest::new(src_id.clone(), "/fork"),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(3).await;
            let fork_id = serde_json::from_slice::<serde_json::Value>(&payloads[2]).unwrap()
                ["sessionId"]
                .as_str()
                .unwrap()
                .to_string();

            // Prompt the fork — must succeed using the inherited key.
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let prompt_subj = format!("acp.session.{fork_id}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(fork_id.clone(), vec![ContentBlock::from("q")]),
                "r.prompt.fork",
            );
            h.expect_n_publishes(4).await;

            assert_eq!(
                h.http.last_api_key(),
                "inherited-key",
                "fork must inherit source's api_key and use it when prompting"
            );
        })
        .await;
}

// ── initialize: embeddedContext capability ────────────────────────────────────

#[tokio::test]
async fn initialize_reports_embedded_context_true_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.global(
                "acp.agent.initialize",
                InitializeRequest::new(ProtocolVersion::LATEST),
                "r.init.ctx",
            );
            let payloads = h.expect_n_publishes(1).await;
            let resp: InitializeResponse = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                resp.agent_capabilities.prompt_capabilities.embedded_context,
                "agent must advertise embeddedContext=true to allow Resource blocks in prompts"
            );
        })
        .await;
}

// ── pending key consumed after first new_session ──────────────────────────────

#[tokio::test]
async fn pending_key_consumed_after_first_new_session() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // No global key — all key supply comes from authenticate.
            let h = Harness::with_api_key("");

            let mut meta = serde_json::Map::new();
            meta.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!("user-key"));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("openrouter-api-key").meta(meta),
                "r.auth",
            );
            h.expect_n_publishes(1).await;

            // First new_session consumes the pending key.
            let sid1 = create_session(&h).await;

            // Second new_session — pending key already taken, no global key.
            let sid2 = create_session(&h).await;

            // Prompt session1 → key available, succeeds.
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let p1 = format!("acp.session.{sid1}.agent.prompt");
            h.session_req(
                &p1,
                PromptRequest::new(sid1.clone(), vec![ContentBlock::from("q")]),
                "r.prompt.s1",
            );
            h.expect_n_publishes(4).await;

            // Prompt session2 → no key at all, must return ACP error.
            let p2 = format!("acp.session.{sid2}.agent.prompt");
            h.session_req(
                &p2,
                PromptRequest::new(sid2.clone(), vec![ContentBlock::from("q")]),
                "r.prompt.s2",
            );
            let payloads = h.expect_n_publishes(5).await;
            let val: serde_json::Value = serde_json::from_slice(payloads.last().unwrap()).unwrap();
            assert!(
                val.get("code").is_some(),
                "prompting a session with no key must return ACP error: {val}"
            );
        })
        .await;
}

// ── authenticate twice: last key wins ────────────────────────────────────────

#[tokio::test]
async fn authenticate_called_twice_last_key_wins() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::with_api_key("");

            // First authenticate — key1 is set as pending.
            let mut meta1 = serde_json::Map::new();
            meta1.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!("key1"));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("openrouter-api-key").meta(meta1),
                "r.auth1",
            );
            h.expect_n_publishes(1).await;

            // Second authenticate — key2 replaces key1.
            let mut meta2 = serde_json::Map::new();
            meta2.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!("key2"));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("openrouter-api-key").meta(meta2),
                "r.auth2",
            );
            h.expect_n_publishes(2).await;

            // new_session consumes the pending key (should be key2).
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.prompt",
            );
            h.expect_n_publishes(4).await;

            assert_eq!(
                h.http.last_api_key(),
                "key2",
                "second authenticate must replace the first — session must receive key2"
            );
        })
        .await;
}

// ── fork session has its own cwd in list_sessions ─────────────────────────────

#[tokio::test]
async fn fork_session_cwd_appears_in_list_sessions() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();

            // Create source session with a specific cwd.
            h.global(
                "acp.agent.session.new",
                NewSessionRequest::new("/source-dir"),
                "r.new",
            );
            let payloads = h.expect_n_publishes(1).await;
            let src_id = serde_json::from_slice::<serde_json::Value>(&payloads[0]).unwrap()
                ["sessionId"]
                .as_str()
                .unwrap()
                .to_string();

            // Fork with a different cwd.
            let fork_subj = format!("acp.session.{src_id}.agent.fork");
            h.session_req(
                &fork_subj,
                ForkSessionRequest::new(src_id.clone(), "/fork-dir"),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(2).await;
            let fork_id = serde_json::from_slice::<serde_json::Value>(&payloads[1]).unwrap()
                ["sessionId"]
                .as_str()
                .unwrap()
                .to_string();

            h.global("acp.agent.session.list", ListSessionsRequest::new(), "r.list");
            let payloads = h.expect_n_publishes(3).await;
            let resp: ListSessionsResponse = serde_json::from_slice(&payloads[2]).unwrap();

            let src_info = resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == src_id)
                .expect("source session must appear in list");
            assert_eq!(
                src_info.cwd.to_string_lossy(),
                "/source-dir",
                "source session must keep its own cwd"
            );

            let fork_info = resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == fork_id)
                .expect("fork session must appear in list");
            assert_eq!(
                fork_info.cwd.to_string_lossy(),
                "/fork-dir",
                "fork session must have its own cwd from the fork request"
            );
        })
        .await;
}

// ── empty ToolCallsReady breaks with EndTurn, no second HTTP call ─────────────

#[tokio::test]
async fn empty_tool_calls_ready_breaks_with_end_turn_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::ToolCallsReady { calls: vec![] }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.empty_tools",
            );

            let payloads = h.expect_n_publishes(2).await;
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "empty ToolCallsReady must resolve to EndTurn: {:?}",
                resp.stop_reason
            );
            let call_count = h.http.recorded_models.lock().unwrap().len();
            assert_eq!(call_count, 1, "empty tool calls must not trigger a second HTTP call");
        })
        .await;
}

// ── bash not in tool list without execution backend ───────────────────────────

#[tokio::test]
async fn bash_not_in_tool_list_without_execution_backend() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::TextDelta { text: "hi".to_string() }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hello")]),
                "r.no_bash",
            );
            h.expect_n_publishes(2).await;

            let tool_names = h.http.last_tool_names();
            assert!(
                !tool_names.iter().any(|n| n == "bash"),
                "bash must not appear in tool list when no execution backend is configured: {tool_names:?}"
            );
        })
        .await;
}

// ── malformed JSON tool arguments do not crash ────────────────────────────────

#[tokio::test]
async fn tool_dispatch_with_malformed_json_does_not_crash_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_bad".to_string(),
                    name: "list_directory".to_string(),
                    arguments: "NOT_VALID_JSON".to_string(),
                }],
            }]);
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "done".to_string() }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.bad_json",
            );

            let payloads = h.expect_n_publishes(2).await;
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "malformed JSON args must not crash the agent: {:?}",
                resp.stop_reason
            );
            let call_count = h.http.recorded_models.lock().unwrap().len();
            assert_eq!(call_count, 2, "tool round-trip must still complete despite malformed args");
        })
        .await;
}

// ── fetch_url egress blocked completes without crash ─────────────────────────

#[tokio::test]
async fn fetch_url_egress_blocked_completes_without_crash() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_egress".to_string(),
                    name: "fetch_url".to_string(),
                    arguments: r#"{"url":"http://169.254.169.254/latest/meta-data/"}"#.to_string(),
                }],
            }]);
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "done".to_string() }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("fetch metadata")]),
                "r.egress",
            );

            let payloads = h.expect_n_publishes(2).await;
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "egress-blocked fetch_url must complete with EndTurn: {:?}",
                resp.stop_reason
            );
            let call_count = h.http.recorded_models.lock().unwrap().len();
            assert_eq!(call_count, 2, "blocked egress must still produce a tool result and a follow-up call");
        })
        .await;
}

// ── env_lock helper for env-var tests ─────────────────────────────────────────

static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.get_or_init(|| std::sync::Mutex::new(())).lock().unwrap()
}

// ── set_session_config_option response has updated config_options ─────────────

#[tokio::test]
async fn set_session_config_option_response_has_updated_config_options() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;
            let before = h.nats.published_payloads().len();

            let set_subj = format!("acp.session.{sid}.agent.set_config_option");
            h.session_req(
                &set_subj,
                SetSessionConfigOptionRequest::new(sid.clone(), "read_file", "disabled"),
                "r.setcfg",
            );
            let payloads = h.expect_n_publishes(before + 1).await;
            let resp: SetSessionConfigOptionResponse =
                serde_json::from_slice(payloads.last().unwrap()).unwrap();

            let read_file_opt = resp
                .config_options
                .iter()
                .find(|o| o.id.to_string() == "read_file")
                .expect("read_file must be in config_options");
            let current = match &read_file_opt.kind {
                SessionConfigKind::Select(s) => s.current_value.to_string(),
                _ => String::new(),
            };
            assert_eq!(current, "disabled", "response must reflect the updated value immediately");
        })
        .await;
}

// ── new_session: all config_options enabled by default ───────────────────────

#[tokio::test]
async fn new_session_all_config_options_enabled_by_default() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.global(
                "acp.agent.session.new",
                NewSessionRequest::new("/"),
                "r.cfg_default",
            );
            let payloads = h.expect_n_publishes(1).await;
            let resp: NewSessionResponse = serde_json::from_slice(&payloads[0]).unwrap();
            let config_options = resp.config_options.unwrap_or_default();
            let all_defs = trogon_tools::all_tool_defs();
            assert_eq!(
                config_options.len(),
                all_defs.len(),
                "must have one config_option per trogon-tool"
            );
            for opt in &config_options {
                let current = match &opt.kind {
                    SessionConfigKind::Select(s) => s.current_value.to_string(),
                    _ => String::new(),
                };
                assert_eq!(current, "enabled", "all tools must be enabled by default in new_session");
            }
        })
        .await;
}

// ── response exceeding max_response_bytes stops early via nats ────────────────

#[tokio::test]
async fn response_exceeding_size_limit_stops_early_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let nats = MockNatsClient::new();
            let http = TestHttpClient::new();
            let notifier = TestNotifier::new();
            let global_tx = nats.inject_messages();
            let session_tx = nats.inject_messages();

            // Two deltas: "hello" (5 bytes) + " world!!!!" (10 bytes) → total 15 > limit 10.
            http.push(vec![
                OpenRouterEvent::TextDelta { text: "hello".to_string() },
                OpenRouterEvent::TextDelta { text: " world!!!!".to_string() },
            ]);

            let prefix = AcpPrefix::new("acp").unwrap();
            let agent =
                OpenRouterAgent::with_deps(notifier, "test-model", "test-key", http)
                    .with_max_response_bytes(10);
            let (_, io_task) =
                AgentSideNatsConnection::new(agent, nats.clone(), prefix, |fut| {
                    tokio::task::spawn_local(fut);
                });
            tokio::task::spawn_local(async move { let _ = io_task.await; });

            inject_req(&global_tx, "acp.agent.session.new", NewSessionRequest::new("/"), "r.new");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 1 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: session.new");
                tokio::task::yield_now().await;
            }
            let sid = {
                let v: serde_json::Value =
                    serde_json::from_slice(&nats.published_payloads()[0]).unwrap();
                v["sessionId"].as_str().unwrap().to_string()
            };

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            inject_req(
                &session_tx,
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.prompt",
            );
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 2 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: prompt response");
                tokio::task::yield_now().await;
            }
            let payloads = nats.published_payloads();
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "size-limited stream must still complete with EndTurn: {:?}",
                resp.stop_reason
            );
        })
        .await;
}

// ── available_models from env appear in new_session ───────────────────────────

#[tokio::test]
async fn available_models_from_env_appear_in_new_session() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let _env_guard = env_lock();
            unsafe {
                std::env::set_var(
                    "OPENROUTER_MODELS",
                    "custom/model-a:Custom Model A,custom/model-b:Custom Model B",
                );
            }
            let h = Harness::new();
            unsafe { std::env::remove_var("OPENROUTER_MODELS"); }
            drop(_env_guard);

            h.global("acp.agent.session.new", NewSessionRequest::new("/"), "r.models");
            let payloads = h.expect_n_publishes(1).await;
            let resp: NewSessionResponse = serde_json::from_slice(&payloads[0]).unwrap();
            let models = resp.models.expect("new_session must return model state");
            let ids: Vec<&str> = models
                .available_models
                .iter()
                .map(|m| m.model_id.0.as_ref())
                .collect();
            assert!(
                ids.contains(&"custom/model-a"),
                "custom/model-a must appear in available_models: {ids:?}"
            );
            assert!(
                ids.contains(&"custom/model-b"),
                "custom/model-b must appear in available_models: {ids:?}"
            );
        })
        .await;
}

// ── fork inherits system_prompt — sent to HTTP client on next prompt ──────────

#[tokio::test]
async fn fork_inherits_system_prompt_sent_to_http_client() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let nats = MockNatsClient::new();
            let http = TestHttpClient::new();
            let notifier = TestNotifier::new();
            let global_tx = nats.inject_messages();
            let session_tx = nats.inject_messages();

            http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);

            let prefix = AcpPrefix::new("acp").unwrap();
            let agent =
                OpenRouterAgent::with_deps(notifier, "test-model", "test-key", http.clone())
                    .with_system_prompt("Be helpful.");
            let (_, io_task) =
                AgentSideNatsConnection::new(agent, nats.clone(), prefix, |fut| {
                    tokio::task::spawn_local(fut);
                });
            tokio::task::spawn_local(async move { let _ = io_task.await; });

            // Create source session
            inject_req(&global_tx, "acp.agent.session.new", NewSessionRequest::new("/src"), "r.src");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 1 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: source session.new");
                tokio::task::yield_now().await;
            }
            let src_id = {
                let v: serde_json::Value =
                    serde_json::from_slice(&nats.published_payloads()[0]).unwrap();
                v["sessionId"].as_str().unwrap().to_string()
            };

            // Fork the source session
            let fork_subj = format!("acp.session.{src_id}.agent.fork");
            inject_req(
                &session_tx,
                &fork_subj,
                ForkSessionRequest::new(src_id.clone(), "/fork"),
                "r.fork",
            );
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 2 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: fork response");
                tokio::task::yield_now().await;
            }
            let fork_id = {
                let v: serde_json::Value =
                    serde_json::from_slice(&nats.published_payloads()[1]).unwrap();
                v["sessionId"].as_str().unwrap().to_string()
            };

            // Prompt the fork — system_prompt must reach chat_stream
            let prompt_subj = format!("acp.session.{fork_id}.agent.prompt");
            inject_req(
                &session_tx,
                &prompt_subj,
                PromptRequest::new(fork_id.clone(), vec![ContentBlock::from("hello")]),
                "r.prompt",
            );
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 3 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: fork prompt response");
                tokio::task::yield_now().await;
            }

            let msgs = http.last_messages();
            assert!(!msgs.is_empty(), "at least one message must be sent to chat_stream");
            assert_eq!(
                msgs[0].role, "system",
                "first wire message must be system role; got: {:?}", msgs[0].role
            );
            assert!(
                msgs[0].content.contains("Be helpful."),
                "system message must carry the inherited prompt; got: {:?}", msgs[0].content
            );
        })
        .await;
}

// ── initialize: auth_methods when no global key ───────────────────────────────

#[tokio::test]
async fn initialize_without_global_key_has_single_auth_method() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::with_api_key(""); // no global key
            h.global(
                "acp.agent.initialize",
                InitializeRequest::new(ProtocolVersion::LATEST),
                "r.init",
            );
            let payloads = h.expect_n_publishes(1).await;
            let resp: InitializeResponse = serde_json::from_slice(&payloads[0]).unwrap();
            assert_eq!(
                resp.auth_methods.len(),
                1,
                "without a global key exactly one auth method must be offered: {:?}",
                resp.auth_methods
            );
            let id = match &resp.auth_methods[0] {
                AuthMethod::EnvVar(m) => m.id.0.as_ref().to_string(),
                other => panic!("expected EnvVar auth method, got {other:?}"),
            };
            assert_eq!(id, "openrouter-api-key");
        })
        .await;
}

// ── initialize: auth_methods when global key is present ──────────────────────

#[tokio::test]
async fn initialize_with_global_key_has_two_auth_methods() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new(); // global key = "test-key"
            h.global(
                "acp.agent.initialize",
                InitializeRequest::new(ProtocolVersion::LATEST),
                "r.init",
            );
            let payloads = h.expect_n_publishes(1).await;
            let resp: InitializeResponse = serde_json::from_slice(&payloads[0]).unwrap();
            assert_eq!(
                resp.auth_methods.len(),
                2,
                "with a global key both env-var and agent methods must be offered: {:?}",
                resp.auth_methods
            );
            let ids: Vec<String> = resp.auth_methods.iter().map(|m| match m {
                AuthMethod::EnvVar(e) => e.id.0.as_ref().to_string(),
                AuthMethod::Agent(a) => a.id.0.as_ref().to_string(),
                _ => "other".to_string(),
            }).collect();
            assert!(ids.contains(&"openrouter-api-key".to_string()), "env-var method missing: {ids:?}");
            assert!(ids.contains(&"agent".to_string()), "agent method missing: {ids:?}");
        })
        .await;
}

// ── initialize: agent_info name ───────────────────────────────────────────────

#[tokio::test]
async fn initialize_agent_info_name_is_correct() {
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
            let info = resp.agent_info.expect("agent_info must be set in initialize response");
            assert_eq!(
                info.name, "trogon-openrouter-runner",
                "agent_info.name must match the crate name"
            );
        })
        .await;
}

// ── initialize: full session capabilities ─────────────────────────────────────

#[tokio::test]
async fn initialize_session_capabilities_are_complete() {
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
            let caps = &resp.agent_capabilities.session_capabilities;
            assert!(caps.fork.is_some(),   "fork capability must be declared");
            assert!(caps.list.is_some(),   "list capability must be declared");
            assert!(caps.resume.is_some(), "resume capability must be declared");
            assert!(caps.close.is_some(),  "close capability must be declared");
        })
        .await;
}

// ── prompt: two text blocks joined with newline ───────────────────────────────

#[tokio::test]
async fn prompt_two_text_blocks_joined_with_newline_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(
                    sid.clone(),
                    vec![
                        ContentBlock::from("first".to_string()),
                        ContentBlock::from("second".to_string()),
                    ],
                ),
                "r.two_blocks",
            );
            h.expect_n_publishes(2).await;

            let msgs = h.http.last_messages();
            let user_msg = msgs.iter().find(|m| m.role == "user")
                .expect("user message must be present");
            assert_eq!(
                user_msg.content, "first\nsecond",
                "two text blocks must be joined with newline"
            );
        })
        .await;
}

// ── prompt: ResourceLink formatted as [Resource: name | uri] ─────────────────

#[tokio::test]
async fn prompt_resource_link_formatted_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(
                    sid.clone(),
                    vec![ContentBlock::ResourceLink(ResourceLink::new(
                        "my-file.txt",
                        "file:///workspace/my-file.txt",
                    ))],
                ),
                "r.resource_link",
            );
            h.expect_n_publishes(2).await;

            let msgs = h.http.last_messages();
            let user_msg = msgs.iter().find(|m| m.role == "user")
                .expect("user message must be present");
            assert_eq!(
                user_msg.content,
                "[Resource: my-file.txt | file:///workspace/my-file.txt]",
                "ResourceLink must be formatted with name and URI"
            );
        })
        .await;
}

// ── prompt: EmbeddedResource TextResourceContents included verbatim ───────────

#[tokio::test]
async fn prompt_embedded_text_resource_verbatim_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
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
                "r.embedded_text",
            );
            h.expect_n_publishes(2).await;

            let msgs = h.http.last_messages();
            let user_msg = msgs.iter().find(|m| m.role == "user")
                .expect("user message must be present");
            assert_eq!(
                user_msg.content, "fn main() {}",
                "TextResourceContents must be included verbatim in wire message"
            );
        })
        .await;
}

// ── prompt: stream timeout → EndTurn ─────────────────────────────────────────

#[tokio::test]
async fn prompt_stream_timeout_returns_end_turn_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let nats = MockNatsClient::new();
            let http = TestHttpClient::new();
            let notifier = TestNotifier::new();
            let global_tx = nats.inject_messages();
            let session_tx = nats.inject_messages();

            // Stream emits one delta then hangs forever — timeout must fire.
            http.push_slow(OpenRouterEvent::TextDelta { text: "partial".to_string() });

            let prefix = AcpPrefix::new("acp").unwrap();
            let agent =
                OpenRouterAgent::with_deps(notifier, "test-model", "test-key", http)
                    .with_prompt_timeout(Duration::from_millis(100));
            let (_, io_task) =
                AgentSideNatsConnection::new(agent, nats.clone(), prefix, |fut| {
                    tokio::task::spawn_local(fut);
                });
            tokio::task::spawn_local(async move { let _ = io_task.await; });

            inject_req(&global_tx, "acp.agent.session.new", NewSessionRequest::new("/"), "r.new");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 1 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: session.new");
                tokio::task::yield_now().await;
            }
            let sid = {
                let v: serde_json::Value =
                    serde_json::from_slice(&nats.published_payloads()[0]).unwrap();
                v["sessionId"].as_str().unwrap().to_string()
            };

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            inject_req(
                &session_tx,
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.prompt",
            );
            // Wait up to 3s for prompt to complete despite the hanging stream.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            loop {
                if nats.published_payloads().len() >= 2 { break; }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "timeout waiting for prompt response after stream timeout"
                );
                tokio::task::yield_now().await;
            }
            let payloads = nats.published_payloads();
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "stream timeout must resolve to EndTurn: {:?}",
                resp.stop_reason
            );
        })
        .await;
}

// ── pending key: second session falls back to global key ──────────────────────

#[tokio::test]
async fn pending_key_consumed_second_session_uses_global_key() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new(); // global key = "test-key"

            // Authenticate with a user-specific key.
            let mut meta = serde_json::Map::new();
            meta.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!("user-key"));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("openrouter-api-key").meta(meta),
                "r.auth",
            );
            h.expect_n_publishes(1).await;

            // First new_session consumes the pending user key.
            let sid1 = create_session(&h).await;
            // Second new_session — pending key already consumed, falls back to global.
            let sid2 = create_session(&h).await;

            // Prompt session 1 — should use "user-key".
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "r1".to_string() }]);
            let p1 = format!("acp.session.{sid1}.agent.prompt");
            h.session_req(&p1, PromptRequest::new(sid1, vec![ContentBlock::from("q")]), "r.p1");
            h.expect_n_publishes(4).await;
            let key1 = h.http.last_api_key();

            // Prompt session 2 — should fall back to "test-key".
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "r2".to_string() }]);
            let p2 = format!("acp.session.{sid2}.agent.prompt");
            h.session_req(&p2, PromptRequest::new(sid2, vec![ContentBlock::from("q")]), "r.p2");
            h.expect_n_publishes(5).await;
            let key2 = h.http.last_api_key();

            assert_eq!(key1, "user-key",  "first session must use the user-provided key");
            assert_eq!(key2, "test-key",  "second session must fall back to the global key");
        })
        .await;
}

// ── fork branchAtIndex:0 clears source history ────────────────────────────────

#[tokio::test]
async fn fork_branch_at_index_zero_clears_history_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let src_id = create_session(&h).await;

            // Prompt source to build up history (user + assistant messages).
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "src answer".to_string() }]);
            let prompt_subj = format!("acp.session.{src_id}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(src_id.clone(), vec![ContentBlock::from("src question")]),
                "r.src_prompt",
            );
            h.expect_n_publishes(2).await;

            // Fork with branchAtIndex:0 — fork must have empty history.
            let fork_subj = format!("acp.session.{src_id}.agent.fork");
            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({"branchAtIndex": 0}),
            ).unwrap();
            h.session_req(
                &fork_subj,
                ForkSessionRequest::new(src_id.clone(), "/fork").meta(meta),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(3).await;
            let fork_id = serde_json::from_slice::<serde_json::Value>(&payloads[2]).unwrap()
                ["sessionId"].as_str().unwrap().to_string();

            // Prompt the fork — wire messages must contain only the new user turn.
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "fork answer".to_string() }]);
            let fork_prompt_subj = format!("acp.session.{fork_id}.agent.prompt");
            h.session_req(
                &fork_prompt_subj,
                PromptRequest::new(fork_id.clone(), vec![ContentBlock::from("fork question")]),
                "r.fork_prompt",
            );
            h.expect_n_publishes(4).await;

            let msgs = h.http.last_messages();
            // With empty history and no system prompt, only the user message is sent.
            assert_eq!(
                msgs.len(),
                1,
                "forked session with branchAtIndex:0 must send only the new user message, not source history: {msgs:?}"
            );
            assert_eq!(msgs[0].role, "user");
            assert_eq!(msgs[0].content, "fork question");
        })
        .await;
}

// ── BlobResourceContents → binary placeholder in wire message ─────────────────

#[tokio::test]
async fn prompt_blob_resource_with_mime_type_formatted_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let mut blob = BlobResourceContents::new("base64data==", "file:///img.png");
            blob.mime_type = Some("image/png".to_string());
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(
                    sid.clone(),
                    vec![ContentBlock::Resource(EmbeddedResource::new(
                        EmbeddedResourceResource::BlobResourceContents(blob),
                    ))],
                ),
                "r.blob_mime",
            );
            h.expect_n_publishes(2).await;

            let msgs = h.http.last_messages();
            let user_msg = msgs.iter().find(|m| m.role == "user")
                .expect("user message must be present");
            assert!(
                user_msg.content.contains("img.png") && user_msg.content.contains("image/png"),
                "BlobResourceContents with mime must include uri and mime type: {:?}",
                user_msg.content
            );
        })
        .await;
}

#[tokio::test]
async fn prompt_blob_resource_without_mime_type_uses_binary_fallback_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let blob = BlobResourceContents::new("base64data==", "file:///data.bin"); // no mime_type
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(
                    sid.clone(),
                    vec![ContentBlock::Resource(EmbeddedResource::new(
                        EmbeddedResourceResource::BlobResourceContents(blob),
                    ))],
                ),
                "r.blob_no_mime",
            );
            h.expect_n_publishes(2).await;

            let msgs = h.http.last_messages();
            let user_msg = msgs.iter().find(|m| m.role == "user")
                .expect("user message must be present");
            assert!(
                user_msg.content.contains("data.bin") && user_msg.content.contains("binary"),
                "BlobResourceContents without mime_type must fall back to 'binary': {:?}",
                user_msg.content
            );
        })
        .await;
}

// ── different message than last turn is not treated as resume ─────────────────

#[tokio::test]
async fn prompt_different_message_not_treated_as_resume_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // First prompt: "ping"
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "pong".to_string() }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("ping")]),
                "r.p1",
            );
            h.expect_n_publishes(2).await;

            // Second prompt: "ping world" (different — must NOT be treated as resume of "ping")
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "pong2".to_string() }]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("ping world")]),
                "r.p2",
            );
            h.expect_n_publishes(3).await;

            // Wire messages for the second call: history[user"ping", asst"pong"] + user"ping world"
            let msgs = h.http.last_messages();
            let user_msgs: Vec<_> = msgs.iter().filter(|m| m.role == "user").collect();
            assert_eq!(
                user_msgs.len(),
                2,
                "second prompt must add a new user turn, not resume the first: {msgs:?}"
            );
            assert_eq!(
                user_msgs[1].content, "ping world",
                "second user message must be 'ping world', not skipped"
            );
        })
        .await;
}

// ── empty content list → EndTurn without error ────────────────────────────────

#[tokio::test]
async fn prompt_empty_content_list_returns_end_turn_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // No events queued — empty response
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![]),
                "r.empty_content",
            );
            let payloads = h.expect_n_publishes(2).await;
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "empty content list must complete with EndTurn: {:?}",
                resp.stop_reason
            );
        })
        .await;
}

// ── Image block is silently skipped ──────────────────────────────────────────

#[tokio::test]
async fn prompt_image_block_silently_skipped_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // No events queued — agent warns but does not error
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(
                    sid.clone(),
                    vec![ContentBlock::Image(ImageContent::new("base64data==", "image/png"))],
                ),
                "r.image",
            );
            let payloads = h.expect_n_publishes(2).await;
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "image-only prompt must not error and must complete with EndTurn: {:?}",
                resp.stop_reason
            );
        })
        .await;
}

// ── Loader stubs (used by loader-* tests) ─────────────────────────────────────

struct FixedAgentLoader {
    sp: Option<String>,
    model: Option<String>,
}

impl AgentLoading for FixedAgentLoader {
    fn load_config<'a>(
        &'a self,
        _: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AgentConfig> + Send + 'a>> {
        let sp = self.sp.clone();
        let model = self.model.clone();
        Box::pin(async move { AgentConfig { skill_ids: vec![], system_prompt: sp, model_id: model } })
    }
}

struct FixedSkillLoader {
    text: Option<String>,
}

impl SkillLoading for FixedSkillLoader {
    fn load<'a>(
        &'a self,
        _: &'a [String],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
        let t = self.text.clone();
        Box::pin(async move { t })
    }
}

// ── Finished event is silently ignored ───────────────────────────────────────

#[tokio::test]
async fn prompt_finished_event_silently_ignored_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![
                OpenRouterEvent::TextDelta { text: "the answer".to_string() },
                OpenRouterEvent::Finished { reason: FinishReason::Stop },
                OpenRouterEvent::Done,
            ]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.finished",
            );
            let payloads = h.expect_n_publishes(2).await;
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "Finished event must be ignored; response must still be EndTurn: {:?}",
                resp.stop_reason
            );
            // Verify text arrived via notifications (not that it was erased).
            assert_eq!(h.http.recorded_models.lock().unwrap().len(), 1,
                "exactly one HTTP call must be made — Finished must not trigger a retry");
        })
        .await;
}

// ── Empty string content block still calls HTTP ───────────────────────────────

#[tokio::test]
async fn prompt_empty_string_still_calls_http_client() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("")]),
                "r.empty_str",
            );
            let payloads = h.expect_n_publishes(2).await;
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "empty string prompt must complete with EndTurn: {:?}",
                resp.stop_reason
            );
            let msgs = h.http.last_messages();
            let user_msg = msgs.iter().find(|m| m.role == "user")
                .expect("must have a user message even for empty string input");
            assert_eq!(user_msg.content, "", "empty string block must produce empty content in wire");
        })
        .await;
}

// ── fork branchAtIndex beyond history length copies full history ──────────────

#[tokio::test]
async fn fork_branch_at_index_beyond_length_copies_full_history_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let src_id = create_session(&h).await;

            // Prompt source to build history (1 user + 1 assistant = 2 history messages).
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "src answer".to_string() }]);
            let src_prompt_subj = format!("acp.session.{src_id}.agent.prompt");
            h.session_req(
                &src_prompt_subj,
                PromptRequest::new(src_id.clone(), vec![ContentBlock::from("src question")]),
                "r.src_prompt",
            );
            h.expect_n_publishes(2).await;

            // Fork with branchAtIndex far beyond history length — must copy full history.
            let fork_subj = format!("acp.session.{src_id}.agent.fork");
            let meta = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(
                serde_json::json!({"branchAtIndex": 9999}),
            ).unwrap();
            h.session_req(
                &fork_subj,
                ForkSessionRequest::new(src_id.clone(), "/fork").meta(meta),
                "r.fork",
            );
            let payloads = h.expect_n_publishes(3).await;
            let fork_id = serde_json::from_slice::<serde_json::Value>(&payloads[2]).unwrap()
                ["sessionId"].as_str().unwrap().to_string();

            // Prompt fork — wire must include the full inherited history + new user turn.
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "fork answer".to_string() }]);
            let fork_prompt_subj = format!("acp.session.{fork_id}.agent.prompt");
            h.session_req(
                &fork_prompt_subj,
                PromptRequest::new(fork_id.clone(), vec![ContentBlock::from("fork question")]),
                "r.fork_prompt",
            );
            h.expect_n_publishes(4).await;

            let msgs = h.http.last_messages();
            // No system prompt in default Harness → wire = [user, assistant] (history) + [new_user].
            assert_eq!(
                msgs.len(),
                3,
                "fork with branchAtIndex=9999 must include full source history (2) + new user (1): {msgs:?}"
            );
            assert_eq!(msgs[0].role, "user");
            assert_eq!(msgs[0].content, "src question");
            assert_eq!(msgs[1].role, "assistant");
            assert_eq!(msgs[2].role, "user");
            assert_eq!(msgs[2].content, "fork question");
        })
        .await;
}

// ── Agent loader: combined system prompt ──────────────────────────────────────

fn make_loader_harness(
    agent_sp: Option<&str>,
    skills: Option<&str>,
    model_id: Option<&str>,
) -> (MockNatsClient, TestHttpClient, UnboundedSender<Message>, UnboundedSender<Message>) {
    let nats = MockNatsClient::new();
    let http = TestHttpClient::new();
    let notifier = TestNotifier::new();
    let global_tx = nats.inject_messages();
    let session_tx = nats.inject_messages();

    let prefix = AcpPrefix::new("acp").unwrap();
    let agent = OpenRouterAgent::with_deps(notifier, "test-model", "test-key", http.clone())
        .with_loaders(
            "agent-id",
            Arc::new(FixedAgentLoader {
                sp: agent_sp.map(str::to_string),
                model: model_id.map(str::to_string),
            }),
            Arc::new(FixedSkillLoader { text: skills.map(str::to_string) }),
        );
    let (_, io_task) = AgentSideNatsConnection::new(agent, nats.clone(), prefix, |fut| {
        tokio::task::spawn_local(fut);
    });
    tokio::task::spawn_local(async move { let _ = io_task.await; });

    (nats, http, global_tx, session_tx)
}

async fn loader_create_session(nats: &MockNatsClient, global_tx: &UnboundedSender<Message>) -> String {
    let before = nats.published_payloads().len();
    inject_req(global_tx, "acp.agent.session.new", NewSessionRequest::new("/tmp"), "r.new");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if nats.published_payloads().len() > before { break; }
        assert!(tokio::time::Instant::now() < deadline, "timeout: session.new");
        tokio::task::yield_now().await;
    }
    let payloads = nats.published_payloads();
    let v: serde_json::Value = serde_json::from_slice(payloads.last().unwrap()).unwrap();
    v["sessionId"].as_str().unwrap().to_string()
}

async fn loader_send_prompt(nats: &MockNatsClient, session_tx: &UnboundedSender<Message>, sid: &str) {
    let before = nats.published_payloads().len();
    let prompt_subj = format!("acp.session.{sid}.agent.prompt");
    inject_req(
        session_tx,
        &prompt_subj,
        PromptRequest::new(sid.to_string(), vec![ContentBlock::from("hi")]),
        "r.prompt",
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if nats.published_payloads().len() > before { break; }
        assert!(tokio::time::Instant::now() < deadline, "timeout: prompt");
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn loader_combined_system_prompt_sent_to_http_client() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (nats, http, global_tx, session_tx) =
                make_loader_harness(Some("Agent prompt."), Some("Skills text."), None);

            http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let sid = loader_create_session(&nats, &global_tx).await;
            loader_send_prompt(&nats, &session_tx, &sid).await;

            let msgs = http.last_messages();
            let sys = msgs.iter().find(|m| m.role == "system")
                .expect("must have a system message when both loader sp and skills are set");
            assert_eq!(
                sys.content,
                "Agent prompt.\n\nSkills text.",
                "combined system prompt must be agent_sp + newlines + skills: {:?}",
                sys.content
            );
        })
        .await;
}

#[tokio::test]
async fn loader_skills_only_system_prompt_sent_to_http_client() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (nats, http, global_tx, session_tx) =
                make_loader_harness(None, Some("Skills only."), None);

            http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let sid = loader_create_session(&nats, &global_tx).await;
            loader_send_prompt(&nats, &session_tx, &sid).await;

            let msgs = http.last_messages();
            let sys = msgs.iter().find(|m| m.role == "system")
                .expect("must have system message when only skills are set");
            assert_eq!(
                sys.content,
                "Skills only.",
                "system prompt must be the skills text when no agent_sp: {:?}",
                sys.content
            );
        })
        .await;
}

#[tokio::test]
async fn loader_agent_prompt_only_sent_to_http_client() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (nats, http, global_tx, session_tx) =
                make_loader_harness(Some("Agent only."), None, None);

            http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let sid = loader_create_session(&nats, &global_tx).await;
            loader_send_prompt(&nats, &session_tx, &sid).await;

            let msgs = http.last_messages();
            let sys = msgs.iter().find(|m| m.role == "system")
                .expect("must have system message when only agent_sp is set");
            assert_eq!(
                sys.content,
                "Agent only.",
                "system prompt must be the agent_sp text when no skills: {:?}",
                sys.content
            );
        })
        .await;
}

#[tokio::test]
async fn loader_neither_produces_no_system_message() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (nats, http, global_tx, session_tx) =
                make_loader_harness(None, None, None);

            http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let sid = loader_create_session(&nats, &global_tx).await;
            loader_send_prompt(&nats, &session_tx, &sid).await;

            let msgs = http.last_messages();
            assert!(
                msgs.iter().all(|m| m.role != "system"),
                "no system message must be sent when both loader sp and skills are absent: {msgs:?}"
            );
        })
        .await;
}

#[tokio::test]
async fn loader_model_id_used_in_wire_request() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (nats, http, global_tx, session_tx) =
                make_loader_harness(None, None, Some("loader-model"));

            http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let sid = loader_create_session(&nats, &global_tx).await;
            loader_send_prompt(&nats, &session_tx, &sid).await;

            assert_eq!(
                http.last_model(),
                "loader-model",
                "loader model_id must override the default model in the HTTP call"
            );
        })
        .await;
}

// ── Auth rejection edge cases ─────────────────────────────────────────────────

#[tokio::test]
async fn authenticate_rejects_empty_key_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let mut meta = serde_json::Map::new();
            meta.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!(""));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("openrouter-api-key").meta(meta),
                "r.auth",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "empty API key must return an ACP error: {val}"
            );
        })
        .await;
}

#[tokio::test]
async fn authenticate_rejects_missing_meta_key_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("openrouter-api-key").meta(serde_json::Map::new()),
                "r.auth",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "missing OPENROUTER_API_KEY in meta must return an ACP error: {val}"
            );
        })
        .await;
}

#[tokio::test]
async fn authenticate_rejects_non_string_key_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let mut meta = serde_json::Map::new();
            meta.insert("OPENROUTER_API_KEY".to_string(), serde_json::json!(42));
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("openrouter-api-key").meta(meta),
                "r.auth",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "non-string API key must return an ACP error: {val}"
            );
        })
        .await;
}

#[tokio::test]
async fn authenticate_unknown_method_fails_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.global(
                "acp.agent.authenticate",
                AuthenticateRequest::new("unknown-method"),
                "r.auth",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "unknown auth method must return an ACP error: {val}"
            );
        })
        .await;
}

// ── Nonexistent session errors for set_session_mode/model/config_option ───────

#[tokio::test]
async fn set_session_mode_nonexistent_session_fails_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.session_req(
                "acp.session.ghost.agent.set_mode",
                SetSessionModeRequest::new("ghost", "default"),
                "r.err",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "set_session_mode on nonexistent session must return ACP error: {val}"
            );
        })
        .await;
}

#[tokio::test]
async fn set_session_model_nonexistent_session_fails_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.session_req(
                "acp.session.ghost.agent.set_model",
                SetSessionModelRequest::new("ghost", "anthropic/claude-sonnet-4-6"),
                "r.err",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "set_session_model on nonexistent session must return ACP error: {val}"
            );
        })
        .await;
}

#[tokio::test]
async fn set_session_config_option_nonexistent_session_fails_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.session_req(
                "acp.session.ghost.agent.set_config_option",
                SetSessionConfigOptionRequest::new("ghost", "read_file", "enabled"),
                "r.err",
            );
            let payloads = h.expect_n_publishes(1).await;
            let val: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
            assert!(
                val.get("code").is_some(),
                "set_config_option on nonexistent session must return ACP error: {val}"
            );
        })
        .await;
}

// ── Config option invalid values silently ignored ─────────────────────────────

#[tokio::test]
async fn set_session_config_option_unknown_value_id_is_ignored_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.session_req(
                &format!("acp.session.{sid}.agent.set_config_option"),
                SetSessionConfigOptionRequest::new(sid.clone(), "read_file", "maybe"),
                "r.cfg",
            );
            let payloads = h.expect_n_publishes(2).await;

            // Must succeed (no ACP error code).
            let val: serde_json::Value = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                val.get("code").is_none(),
                "unknown value_id must not return an error: {val}"
            );

            // State must be unchanged — read_file must still be enabled.
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.prompt",
            );
            h.expect_n_publishes(3).await;

            let tools = h.http.last_tool_names();
            assert!(
                tools.contains(&"read_file".to_string()),
                "unknown value_id must not change tool state; read_file must still be enabled: {tools:?}"
            );
        })
        .await;
}

#[tokio::test]
async fn set_session_config_option_boolean_value_is_ignored_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Explicitly disable read_file.
            h.session_req(
                &format!("acp.session.{sid}.agent.set_config_option"),
                SetSessionConfigOptionRequest::new(sid.clone(), "read_file", "disabled"),
                "r.disable",
            );
            h.expect_n_publishes(2).await;

            // Send a boolean value — must be silently ignored, not re-enable read_file.
            let mut req = SetSessionConfigOptionRequest::new(sid.clone(), "read_file", "enabled");
            req.value = agent_client_protocol::SessionConfigOptionValue::Boolean { value: true };
            h.session_req(
                &format!("acp.session.{sid}.agent.set_config_option"),
                req,
                "r.bool",
            );
            h.expect_n_publishes(3).await;

            // read_file must still be disabled.
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            h.session_req(
                &format!("acp.session.{sid}.agent.prompt"),
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.prompt",
            );
            h.expect_n_publishes(4).await;

            let tools = h.http.last_tool_names();
            assert!(
                !tools.contains(&"read_file".to_string()),
                "boolean value must be silently ignored; read_file must stay disabled: {tools:?}"
            );
        })
        .await;
}

// ── Prompt without API key returns ACP error ──────────────────────────────────

#[tokio::test]
async fn prompt_fails_without_api_key_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let nats = MockNatsClient::new();
            let http = TestHttpClient::new();
            let notifier = TestNotifier::new();
            let global_tx = nats.inject_messages();
            let session_tx = nats.inject_messages();

            let prefix = AcpPrefix::new("acp").unwrap();
            // No global key — pass empty string which agent treats as absent.
            let agent = OpenRouterAgent::with_deps(notifier, "test-model", "", http);
            let (_, io_task) = AgentSideNatsConnection::new(agent, nats.clone(), prefix, |fut| {
                tokio::task::spawn_local(fut);
            });
            tokio::task::spawn_local(async move { let _ = io_task.await; });

            inject_req(&global_tx, "acp.agent.session.new", NewSessionRequest::new("/"), "r.new");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 1 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: session.new");
                tokio::task::yield_now().await;
            }
            let sid = {
                let v: serde_json::Value =
                    serde_json::from_slice(&nats.published_payloads()[0]).unwrap();
                v["sessionId"].as_str().unwrap().to_string()
            };

            inject_req(
                &session_tx,
                &format!("acp.session.{sid}.agent.prompt"),
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 2 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: prompt");
                tokio::task::yield_now().await;
            }
            let val: serde_json::Value =
                serde_json::from_slice(&nats.published_payloads()[1]).unwrap();
            assert!(
                val.get("code").is_some(),
                "prompt without API key must return an ACP error: {val}"
            );
        })
        .await;
}

// ── Prompt with no HTTP response stores only user message ─────────────────────

#[tokio::test]
async fn prompt_with_no_response_stores_only_user_message_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // HTTP returns empty stream — no assistant message.
            h.http.push(vec![]);
            h.session_req(
                &format!("acp.session.{sid}.agent.prompt"),
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("ping")]),
                "r.prompt",
            );
            h.expect_n_publishes(2).await;

            // Second prompt — wire messages reveal history from the first call.
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            h.session_req(
                &format!("acp.session.{sid}.agent.prompt"),
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("follow-up")]),
                "r.prompt2",
            );
            h.expect_n_publishes(3).await;

            let msgs = h.http.last_messages();
            // Wire = [user "ping"] (history only user, no assistant) + [user "follow-up"] = 2.
            assert_eq!(
                msgs.len(),
                2,
                "empty HTTP response must store only the user message, not an assistant message: {msgs:?}"
            );
            assert_eq!(msgs[0].role, "user");
            assert_eq!(msgs[0].content, "ping");
            assert_eq!(msgs[1].role, "user");
            assert_eq!(msgs[1].content, "follow-up");
        })
        .await;
}

// ── Size limit: exactly at limit does not stop ────────────────────────────────

#[tokio::test]
async fn response_exactly_at_size_limit_does_not_stop_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let nats = MockNatsClient::new();
            let http = TestHttpClient::new();
            let notifier = TestNotifier::new();
            let global_tx = nats.inject_messages();
            let session_tx = nats.inject_messages();

            // "hello" is exactly 5 bytes — at the limit, the guard uses `>`, not `>=`.
            http.push(vec![OpenRouterEvent::TextDelta { text: "hello".to_string() }]);

            let prefix = AcpPrefix::new("acp").unwrap();
            let http_clone = http.clone();
            let agent = OpenRouterAgent::with_deps(notifier, "test-model", "test-key", http_clone)
                .with_max_response_bytes(5);
            let (_, io_task) = AgentSideNatsConnection::new(agent, nats.clone(), prefix, |fut| {
                tokio::task::spawn_local(fut);
            });
            tokio::task::spawn_local(async move { let _ = io_task.await; });

            inject_req(&global_tx, "acp.agent.session.new", NewSessionRequest::new("/"), "r.new");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 1 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: session.new");
                tokio::task::yield_now().await;
            }
            let sid = {
                let v: serde_json::Value =
                    serde_json::from_slice(&nats.published_payloads()[0]).unwrap();
                v["sessionId"].as_str().unwrap().to_string()
            };

            inject_req(
                &session_tx,
                &format!("acp.session.{sid}.agent.prompt"),
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.prompt",
            );
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 2 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: prompt");
                tokio::task::yield_now().await;
            }
            let resp: PromptResponse =
                serde_json::from_slice(&nats.published_payloads()[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "response exactly at limit must complete with EndTurn (guard is >, not >=): {:?}",
                resp.stop_reason
            );
            assert_eq!(
                http.recorded_models.lock().unwrap().len(),
                1,
                "exactly one HTTP call must be made — no early stop at limit"
            );
        })
        .await;
}

// ── Size limit: partial response saved to history ─────────────────────────────

#[tokio::test]
async fn size_limit_guard_saves_partial_to_history_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let nats = MockNatsClient::new();
            let http = TestHttpClient::new();
            let notifier = TestNotifier::new();
            let global_tx = nats.inject_messages();
            let session_tx = nats.inject_messages();

            // "abcd" is 4 bytes > limit of 3 — triggers early stop.
            http.push(vec![OpenRouterEvent::TextDelta { text: "abcd".to_string() }]);

            let prefix = AcpPrefix::new("acp").unwrap();
            let http_clone = http.clone();
            let agent = OpenRouterAgent::with_deps(notifier, "test-model", "test-key", http_clone)
                .with_max_response_bytes(3);
            let (_, io_task) = AgentSideNatsConnection::new(agent, nats.clone(), prefix, |fut| {
                tokio::task::spawn_local(fut);
            });
            tokio::task::spawn_local(async move { let _ = io_task.await; });

            inject_req(&global_tx, "acp.agent.session.new", NewSessionRequest::new("/"), "r.new");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 1 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: session.new");
                tokio::task::yield_now().await;
            }
            let sid = {
                let v: serde_json::Value =
                    serde_json::from_slice(&nats.published_payloads()[0]).unwrap();
                v["sessionId"].as_str().unwrap().to_string()
            };

            inject_req(
                &session_tx,
                &format!("acp.session.{sid}.agent.prompt"),
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.prompt",
            );
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 2 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: prompt");
                tokio::task::yield_now().await;
            }

            // Second prompt to inspect history via wire messages.
            http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            inject_req(
                &session_tx,
                &format!("acp.session.{sid}.agent.prompt"),
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("next")]),
                "r.prompt2",
            );
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 3 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: prompt2");
                tokio::task::yield_now().await;
            }

            let msgs = http.last_messages();
            // Wire = [user "q"] + [assistant "abcd" (partial)] + [user "next"] = 3 messages.
            assert_eq!(
                msgs.len(),
                3,
                "partial response must be saved to history even when size limit stops the stream: {msgs:?}"
            );
            assert_eq!(msgs[1].role, "assistant");
            assert_eq!(
                msgs[1].content,
                "abcd",
                "partial assistant text must be preserved in history"
            );
        })
        .await;
}

// ── Loader fallback to with_system_prompt ─────────────────────────────────────

#[tokio::test]
async fn loader_falls_back_to_with_system_prompt_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Loader returns no agent_sp and no skills, but agent has with_system_prompt("Base prompt.").
            let nats = MockNatsClient::new();
            let http = TestHttpClient::new();
            let notifier = TestNotifier::new();
            let global_tx = nats.inject_messages();
            let session_tx = nats.inject_messages();

            let prefix = AcpPrefix::new("acp").unwrap();
            let http_clone = http.clone();
            let agent = OpenRouterAgent::with_deps(notifier, "test-model", "test-key", http_clone)
                .with_system_prompt("Base prompt.")
                .with_loaders(
                    "agent-id",
                    Arc::new(FixedAgentLoader { sp: None, model: None }),
                    Arc::new(FixedSkillLoader { text: None }),
                );
            let (_, io_task) = AgentSideNatsConnection::new(agent, nats.clone(), prefix, |fut| {
                tokio::task::spawn_local(fut);
            });
            tokio::task::spawn_local(async move { let _ = io_task.await; });

            inject_req(&global_tx, "acp.agent.session.new", NewSessionRequest::new("/"), "r.new");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 1 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: session.new");
                tokio::task::yield_now().await;
            }
            let sid = {
                let v: serde_json::Value =
                    serde_json::from_slice(&nats.published_payloads()[0]).unwrap();
                v["sessionId"].as_str().unwrap().to_string()
            };

            http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            inject_req(
                &session_tx,
                &format!("acp.session.{sid}.agent.prompt"),
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("hi")]),
                "r.prompt",
            );
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 2 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: prompt");
                tokio::task::yield_now().await;
            }

            let msgs = http.last_messages();
            let sys = msgs.iter().find(|m| m.role == "system")
                .expect("must have system message when loader has no prompt but with_system_prompt is set");
            assert_eq!(
                sys.content,
                "Base prompt.",
                "with_system_prompt must be used as fallback when loader returns no prompt: {:?}",
                sys.content
            );
        })
        .await;
}

// ── Available models env var edge cases ──────────────────────────────────────

#[tokio::test]
async fn available_models_malformed_entry_skipped_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let _env_guard = env_lock();
            unsafe {
                std::env::set_var(
                    "OPENROUTER_MODELS",
                    "good/model-a:Good Model A,no-colon-entry,good/model-b:Good Model B",
                );
            }
            let h = Harness::new();
            unsafe { std::env::remove_var("OPENROUTER_MODELS"); }

            h.global(
                "acp.agent.session.new",
                NewSessionRequest::new("/"),
                "r.new",
            );
            let payloads = h.expect_n_publishes(1).await;
            let resp: NewSessionResponse = serde_json::from_slice(&payloads[0]).unwrap();
            let models = resp.models.expect("must have model state");
            let ids: Vec<&str> = models.available_models.iter()
                .map(|m| m.model_id.0.as_ref())
                .collect();
            assert!(ids.contains(&"good/model-a"), "valid entry must appear: {ids:?}");
            assert!(ids.contains(&"good/model-b"), "valid entry must appear: {ids:?}");
            assert!(
                !ids.contains(&"no-colon-entry"),
                "malformed entry without colon must be skipped: {ids:?}"
            );
        })
        .await;
}

#[tokio::test]
async fn available_models_all_malformed_falls_back_to_defaults_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let _env_guard = env_lock();
            unsafe {
                std::env::set_var("OPENROUTER_MODELS", "no-colon,also-no-colon");
            }
            let h = Harness::new();
            unsafe { std::env::remove_var("OPENROUTER_MODELS"); }

            h.global(
                "acp.agent.session.new",
                NewSessionRequest::new("/"),
                "r.new",
            );
            let payloads = h.expect_n_publishes(1).await;
            let resp: NewSessionResponse = serde_json::from_slice(&payloads[0]).unwrap();
            let models = resp.models.expect("must have model state");
            assert!(
                models.available_models.len() >= 3,
                "all-malformed env must fall back to hardcoded defaults (≥3 models): {:?}",
                models.available_models
            );
            assert!(
                models.available_models.iter().any(|m| m.model_id.0.as_ref().contains("claude")),
                "hardcoded defaults must include a Claude model: {:?}",
                models.available_models
            );
        })
        .await;
}

// ── tool round: second HTTP call contains tool messages ───────────────────────

#[tokio::test]
async fn tool_round_second_http_call_contains_tool_messages_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_abc".to_string(),
                    name: "list_directory".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                }],
            }]);
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "done".to_string() }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("list files")]),
                "r.wire",
            );
            h.expect_n_publishes(2).await;

            let msgs = h.http.last_messages();

            // Second call must include an assistant message with tool_calls
            let asst_tc = msgs.iter().find(|m| m.role == "assistant" && m.tool_calls.is_some())
                .expect("second HTTP call must include assistant message with tool_calls");
            let tc = asst_tc.tool_calls.as_ref().unwrap();
            assert_eq!(tc.len(), 1, "must have exactly one tool call");
            assert_eq!(tc[0].id, "call_abc");
            assert_eq!(tc[0].name, "list_directory");

            // Second call must include a tool-result message
            let tool_result = msgs.iter().find(|m| m.tool_call_id.is_some())
                .expect("second HTTP call must include a tool result message");
            assert_eq!(
                tool_result.tool_call_id.as_deref(),
                Some("call_abc"),
                "tool result must reference the call id"
            );
        })
        .await;
}

// ── multiple tool calls in one ToolCallsReady, all dispatched ─────────────────

#[tokio::test]
async fn multiple_tool_calls_in_one_round_all_dispatched_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![
                    AssembledToolCall {
                        id: "call_1".to_string(),
                        name: "list_directory".to_string(),
                        arguments: r#"{"path":"."}"#.to_string(),
                    },
                    AssembledToolCall {
                        id: "call_2".to_string(),
                        name: "read_file".to_string(),
                        arguments: r#"{"path":"README.md"}"#.to_string(),
                    },
                ],
            }]);
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "done".to_string() }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("run both")]),
                "r.multi",
            );
            h.expect_n_publishes(2).await;

            let msgs = h.http.last_messages();

            // Must have two separate tool result messages
            let tool_results: Vec<_> = msgs.iter().filter(|m| m.tool_call_id.is_some()).collect();
            assert_eq!(
                tool_results.len(),
                2,
                "both tool calls must produce a tool result in the second HTTP call: {msgs:?}"
            );
            let ids: Vec<Option<&str>> = tool_results.iter().map(|m| m.tool_call_id.as_deref()).collect();
            assert!(ids.contains(&Some("call_1")), "call_1 result must be present");
            assert!(ids.contains(&Some("call_2")), "call_2 result must be present");
        })
        .await;
}

// ── two consecutive tool rounds in a single prompt ────────────────────────────

#[tokio::test]
async fn two_consecutive_tool_rounds_complete_with_end_turn_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Round 1: model calls list_directory
            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_r1".to_string(),
                    name: "list_directory".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                }],
            }]);
            // Round 2: model calls read_file based on round-1 output
            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_r2".to_string(),
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"Cargo.toml"}"#.to_string(),
                }],
            }]);
            // Final: model produces text after both tool rounds
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "all done".to_string() }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("explore the project")]),
                "r.two_rounds",
            );
            let payloads = h.expect_n_publishes(2).await;

            // Prompt must resolve to EndTurn (not Cancelled or error)
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "two consecutive tool rounds must resolve to EndTurn: {:?}",
                resp.stop_reason
            );

            // Exactly 3 HTTP calls must have been made
            let call_count = h.http.recorded_models.lock().unwrap().len();
            assert_eq!(call_count, 3, "must make 3 HTTP calls for two tool rounds + final answer");

            // Third HTTP call must carry both tool rounds in its wire messages
            let third_call_msgs = h.http.recorded_messages.lock().unwrap()[2].clone();
            let tool_calls_msgs: Vec<_> = third_call_msgs.iter()
                .filter(|m| m.tool_calls.is_some())
                .collect();
            assert_eq!(
                tool_calls_msgs.len(), 2,
                "third HTTP call must include 2 assistant tool_calls messages (one per round): {third_call_msgs:?}"
            );
            let tool_result_msgs: Vec<_> = third_call_msgs.iter()
                .filter(|m| m.tool_call_id.is_some())
                .collect();
            assert_eq!(
                tool_result_msgs.len(), 2,
                "third HTTP call must include 2 tool result messages (one per round): {third_call_msgs:?}"
            );
            let result_ids: Vec<Option<&str>> = tool_result_msgs.iter()
                .map(|m| m.tool_call_id.as_deref())
                .collect();
            assert!(result_ids.contains(&Some("call_r1")), "round-1 tool result must be present");
            assert!(result_ids.contains(&Some("call_r2")), "round-2 tool result must be present");

            // Four tool notifications: InProgress+Completed for each round
            let notes = h.notifier.notifications.lock().unwrap();
            let tool_call_count = notes.iter()
                .filter(|n| matches!(&n.update, SessionUpdate::ToolCall(_)))
                .count();
            let tool_call_update_count = notes.iter()
                .filter(|n| matches!(&n.update, SessionUpdate::ToolCallUpdate(_)))
                .count();
            assert_eq!(tool_call_count, 2, "must emit 2 ToolCall (InProgress) notifications for 2 rounds");
            assert_eq!(tool_call_update_count, 2, "must emit 2 ToolCallUpdate (Completed) notifications for 2 rounds");
        })
        .await;
}

// ── fetch_url blocked by egress policy ───────────────────────────────────────

#[tokio::test]
async fn fetch_url_egress_blocked_error_message_in_tool_result_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_egress".to_string(),
                    name: "fetch_url".to_string(),
                    arguments: r#"{"url":"http://169.254.169.254/latest/meta-data/"}"#.to_string(),
                }],
            }]);
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "done".to_string() }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("fetch metadata")]),
                "r.egress",
            );
            h.expect_n_publishes(2).await;

            let msgs = h.http.last_messages();
            let tool_result = msgs.iter().find(|m| m.tool_call_id.is_some())
                .expect("egress-blocked call must still produce a tool result message");
            assert!(
                tool_result.content.contains("blocked by egress policy"),
                "tool result must contain egress error, got: {}",
                tool_result.content
            );
        })
        .await;
}

// ── notification order: InProgress before Completed ──────────────────────────

#[tokio::test]
async fn tool_notification_in_progress_before_completed_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_ord".to_string(),
                    name: "list_directory".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                }],
            }]);
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "done".to_string() }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.order",
            );
            h.expect_n_publishes(2).await;

            let notes = h.notifier.notifications.lock().unwrap();
            let in_progress_pos = notes
                .iter()
                .position(|n| matches!(&n.update, SessionUpdate::ToolCall(_)))
                .expect("must have InProgress (ToolCall) notification");
            let completed_pos = notes
                .iter()
                .position(|n| matches!(&n.update, SessionUpdate::ToolCallUpdate(_)))
                .expect("must have Completed (ToolCallUpdate) notification");
            assert!(
                in_progress_pos < completed_pos,
                "InProgress notification must precede Completed: in_progress={in_progress_pos}, completed={completed_pos}"
            );
        })
        .await;
}

// ── text notification sent after tool round completes ────────────────────────

#[tokio::test]
async fn text_notification_sent_after_tool_round_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_txt".to_string(),
                    name: "list_directory".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                }],
            }]);
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "final answer".to_string() }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.txt_after",
            );
            h.expect_n_publishes(2).await;

            let notes = h.notifier.notifications.lock().unwrap();
            let last_tool_update_pos = notes
                .iter()
                .rposition(|n| matches!(&n.update, SessionUpdate::ToolCallUpdate(_)))
                .expect("must have at least one ToolCallUpdate notification");
            let text_after = notes[last_tool_update_pos + 1..]
                .iter()
                .any(|n| matches!(&n.update, SessionUpdate::AgentMessageChunk(_)));
            assert!(
                text_after,
                "a TextUpdate notification must follow the last ToolCallUpdate: {notes:?}"
            );
        })
        .await;
}

// ── trim_history never orphans tool result messages ───────────────────────────

#[tokio::test]
async fn trim_history_does_not_orphan_tool_result_messages_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // max=2 forces aggressive trimming: after each prompt the history is
            // cut to 2 messages, so we can verify no orphaned tool results survive.
            let _env_guard = env_lock();
            unsafe { std::env::set_var("OPENROUTER_MAX_HISTORY_MESSAGES", "2"); }
            let h = Harness::new();
            unsafe { std::env::remove_var("OPENROUTER_MAX_HISTORY_MESSAGES"); }

            let sid = create_session(&h).await;

            // Prompt 1: simple exchange, establishes baseline history
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "pong".to_string() }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("ping")]),
                "r.trim1",
            );
            h.expect_n_publishes(2).await;

            // Prompt 2: tool round — produces assistant tool_calls + tool result in history
            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_trim".to_string(),
                    name: "list_directory".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                }],
            }]);
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "done".to_string() }]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("list files")]),
                "r.trim2",
            );
            h.expect_n_publishes(3).await;

            // Prompt 3: simple — forces another trim; verify second HTTP call wire messages
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("last")]),
                "r.trim3",
            );
            h.expect_n_publishes(4).await;

            let msgs = h.http.last_messages();
            // Invariant: no tool_call_id without a preceding tool_calls in the same wire batch
            let has_orphaned_tool_result = msgs.iter().any(|m| {
                m.tool_call_id.is_some()
                    && !msgs.iter().any(|other| other.tool_calls.is_some())
            });
            assert!(
                !has_orphaned_tool_result,
                "trim must not leave orphaned tool result messages without matching tool_calls: {msgs:?}"
            );
        })
        .await;
}

// ── tool-round history preserved into next prompt's wire messages ─────────────

#[tokio::test]
async fn tool_round_history_preserved_for_next_prompt_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Prompt 1: tool round then final answer
            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_hist".to_string(),
                    name: "list_directory".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                }],
            }]);
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "done".to_string() }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("p1")]),
                "r.p1",
            );
            h.expect_n_publishes(2).await;

            // Prompt 2: simple exchange; last_messages() will be its wire batch
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("p2")]),
                "r.p2",
            );
            h.expect_n_publishes(3).await;

            let msgs = h.http.last_messages();

            // The wire batch for prompt 2 must contain the complete tool-round
            // from prompt 1 — verifies in-memory history is not corrupted by
            // build_snapshot's filtering (which only affects the persisted snapshot).
            assert!(
                msgs.iter().any(|m| m.role == "assistant" && m.tool_calls.is_some()),
                "wire messages for second prompt must include assistant tool_calls from history: {msgs:?}"
            );
            assert!(
                msgs.iter().any(|m| m.tool_call_id.as_deref() == Some("call_hist")),
                "wire messages for second prompt must include tool result from history: {msgs:?}"
            );
            let last_user = msgs.iter().rfind(|m| m.role == "user")
                .expect("must have a user message");
            assert_eq!(
                last_user.content, "p2",
                "last user message must be the second prompt, not the first: {msgs:?}"
            );
        })
        .await;
}

// ── UsageUpdate notification fires in the final round after a tool loop ───────

#[tokio::test]
async fn usage_update_notification_fires_after_tool_round_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Call 1: tool round
            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_usage".to_string(),
                    name: "list_directory".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                }],
            }]);
            // Call 2: final answer with usage event
            h.http.push(vec![
                OpenRouterEvent::Usage { prompt_tokens: 20, completion_tokens: 10, cache_read_tokens: 0, cache_creation_tokens: 0 },
                OpenRouterEvent::TextDelta { text: "done".to_string() },
            ]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.usage_after_tool",
            );
            h.expect_n_publishes(2).await;

            let notes = h.notifier.notifications.lock().unwrap();
            let has_usage = notes
                .iter()
                .any(|n| matches!(&n.update, SessionUpdate::UsageUpdate(_)));
            assert!(
                has_usage,
                "UsageUpdate notification must be emitted even when usage arrives in the second HTTP call: {notes:?}"
            );
        })
        .await;
}

// ── per-chunk timeout is preserved in the second HTTP call of a tool loop ─────

#[tokio::test]
async fn stream_timeout_in_second_http_call_of_tool_round_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let nats = MockNatsClient::new();
            let http = TestHttpClient::new();
            let notifier = TestNotifier::new();
            let global_tx = nats.inject_messages();
            let session_tx = nats.inject_messages();

            // Call 1: returns tool call immediately
            http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_slow".to_string(),
                    name: "list_directory".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                }],
            }]);
            // Call 2: emits one delta then hangs — timeout must fire
            http.push_slow(OpenRouterEvent::TextDelta { text: "partial".to_string() });

            let prefix = AcpPrefix::new("acp").unwrap();
            let agent = OpenRouterAgent::with_deps(notifier, "test-model", "test-key", http)
                .with_prompt_timeout(Duration::from_millis(100));
            let (_, io_task) =
                AgentSideNatsConnection::new(agent, nats.clone(), prefix, |fut| {
                    tokio::task::spawn_local(fut);
                });
            tokio::task::spawn_local(async move { let _ = io_task.await; });

            inject_req(&global_tx, "acp.agent.session.new", NewSessionRequest::new("/"), "r.new");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 1 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: session.new");
                tokio::task::yield_now().await;
            }
            let sid = {
                let v: serde_json::Value =
                    serde_json::from_slice(&nats.published_payloads()[0]).unwrap();
                v["sessionId"].as_str().unwrap().to_string()
            };

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            inject_req(
                &session_tx,
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.prompt",
            );
            // Wait up to 3s for the second-call timeout to fire and produce a response.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            loop {
                if nats.published_payloads().len() >= 2 { break; }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "timeout waiting for prompt response after second-call stream timeout"
                );
                tokio::task::yield_now().await;
            }
            let payloads = nats.published_payloads();
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "second-call stream timeout must resolve to EndTurn: {:?}",
                resp.stop_reason
            );
        })
        .await;
}

// ── partial text before ToolCallsReady not stored as assistant message ────────

#[tokio::test]
async fn partial_text_before_tool_calls_not_in_next_wire_messages_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Call 1: partial text then tool call — partial text must NOT end up
            // as an assistant message in history because assistant_text.clear() runs
            // at the top of each outer loop iteration.
            h.http.push(vec![
                OpenRouterEvent::TextDelta { text: "pre-tool-text".to_string() },
                OpenRouterEvent::ToolCallsReady {
                    calls: vec![AssembledToolCall {
                        id: "call_pt".to_string(),
                        name: "list_directory".to_string(),
                        arguments: r#"{"path":"."}"#.to_string(),
                    }],
                },
            ]);
            // Call 2: final answer
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "final".to_string() }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.partial",
            );
            h.expect_n_publishes(2).await;

            // The messages sent in call 2 are the wire batch. The partial text
            // "pre-tool-text" must not appear there as a standalone assistant message.
            let msgs = h.http.last_messages();
            let partial_as_assistant = msgs.iter().any(|m| {
                m.role == "assistant"
                    && m.tool_calls.is_none()
                    && m.content.contains("pre-tool-text")
            });
            assert!(
                !partial_as_assistant,
                "partial text accumulated before ToolCallsReady must not be stored as an assistant message: {msgs:?}"
            );
        })
        .await;
}

// ── max_response_bytes guard fires in the final round of a tool loop ──────────

#[tokio::test]
async fn max_response_bytes_guard_applies_after_tool_round_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let nats = MockNatsClient::new();
            let http = TestHttpClient::new();
            let notifier = TestNotifier::new();
            let global_tx = nats.inject_messages();
            let session_tx = nats.inject_messages();

            // Call 1: tool round
            http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_sz".to_string(),
                    name: "list_directory".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                }],
            }]);
            // Call 2: two deltas totaling 15 bytes — above the 10-byte limit.
            // assistant_text resets to "" at the start of each outer loop iteration,
            // so the guard measures only call-2 text, not any text from call 1.
            http.push(vec![
                OpenRouterEvent::TextDelta { text: "hello".to_string() },
                OpenRouterEvent::TextDelta { text: " world!!!!".to_string() },
            ]);

            let prefix = AcpPrefix::new("acp").unwrap();
            let agent = OpenRouterAgent::with_deps(notifier, "test-model", "test-key", http)
                .with_max_response_bytes(10);
            let (_, io_task) =
                AgentSideNatsConnection::new(agent, nats.clone(), prefix, |fut| {
                    tokio::task::spawn_local(fut);
                });
            tokio::task::spawn_local(async move { let _ = io_task.await; });

            inject_req(&global_tx, "acp.agent.session.new", NewSessionRequest::new("/"), "r.new");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 1 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: session.new");
                tokio::task::yield_now().await;
            }
            let sid = {
                let v: serde_json::Value =
                    serde_json::from_slice(&nats.published_payloads()[0]).unwrap();
                v["sessionId"].as_str().unwrap().to_string()
            };

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            inject_req(
                &session_tx,
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.sz",
            );
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                if nats.published_payloads().len() >= 2 { break; }
                assert!(tokio::time::Instant::now() < deadline, "timeout: prompt response");
                tokio::task::yield_now().await;
            }
            let payloads = nats.published_payloads();
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "max_response_bytes guard in final round must produce EndTurn: {:?}",
                resp.stop_reason
            );
        })
        .await;
}

// ── cancel during second HTTP call of tool loop → Cancelled ──────────────────

#[tokio::test]
async fn cancel_during_second_http_call_of_tool_round_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Call 1: tool round completes immediately
            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_cancel".to_string(),
                    name: "list_directory".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                }],
            }]);
            // Call 2: hangs — cancel must interrupt it
            h.http.push_slow(OpenRouterEvent::TextDelta { text: "partial".to_string() });

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.cancel2",
            );

            // Wait for: ToolCall InProgress + ToolCallUpdate Completed + AgentMessageChunk
            // from the second HTTP call. That guarantees we're inside the second streaming
            // loop while the slow stream is still pending.
            h.expect_n_notifications(3).await;

            let cancel_subj = format!("acp.session.{sid}.agent.cancel");
            h.session_notify(&cancel_subj, CancelNotification::new(sid.clone()));

            let payloads = h.expect_n_publishes(2).await;
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::Cancelled),
                "cancel during second HTTP call must produce Cancelled: {:?}",
                resp.stop_reason
            );
        })
        .await;
}

// ── ToolCall InProgress notification has ToolKind::Other for non-bash tools ───

#[tokio::test]
async fn tool_call_inprogress_notification_has_kind_other_for_trogon_tools_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_kind".to_string(),
                    name: "list_directory".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                }],
            }]);
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "done".to_string() }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.kind",
            );
            h.expect_n_publishes(2).await;

            let notes = h.notifier.notifications.lock().unwrap();
            let in_progress = notes.iter().find_map(|n| {
                if let SessionUpdate::ToolCall(tc) = &n.update { Some(tc) } else { None }
            }).expect("must have a ToolCall (InProgress) notification");

            assert_eq!(
                in_progress.kind, ToolKind::Other,
                "non-bash tool must notify with ToolKind::Other, got {:?}",
                in_progress.kind
            );
            assert_eq!(
                in_progress.status,
                ToolCallStatus::InProgress,
                "InProgress notification must have InProgress status"
            );
        })
        .await;
}

// ── ToolCallUpdate Completed notification carries the tool result in raw_output

#[tokio::test]
async fn tool_call_completed_notification_has_raw_output_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_out".to_string(),
                    name: "list_directory".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                }],
            }]);
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "done".to_string() }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.raw_out",
            );
            h.expect_n_publishes(2).await;

            let notes = h.notifier.notifications.lock().unwrap();
            let completed = notes.iter().find_map(|n| {
                if let SessionUpdate::ToolCallUpdate(tcu) = &n.update { Some(tcu) } else { None }
            }).expect("must have a ToolCallUpdate (Completed) notification");

            assert_eq!(
                completed.fields.status,
                Some(ToolCallStatus::Completed),
                "Completed notification must have Completed status"
            );
            assert!(
                completed.fields.raw_output.is_some(),
                "Completed notification must carry raw_output with the tool result"
            );
            // raw_output must be a non-empty string (the actual tool dispatch result)
            let raw = completed.fields.raw_output.as_ref().unwrap();
            assert!(
                raw.is_string(),
                "raw_output must be a JSON string, got: {raw:?}"
            );
            assert!(
                !raw.as_str().unwrap_or("").is_empty(),
                "raw_output string must not be empty"
            );
        })
        .await;
}

// ── Error event in the final round of the tool loop → EndTurn ────────────────

#[tokio::test]
async fn error_event_in_second_http_call_of_tool_round_returns_end_turn_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            // Call 1: tool round
            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_err".to_string(),
                    name: "list_directory".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                }],
            }]);
            // Call 2: Error event — inner loop breaks, assembled_calls stays empty
            // → outer loop hits assembled_calls.is_empty() → EndTurn
            h.http.push(vec![OpenRouterEvent::Error {
                message: "upstream error in round 2".to_string(),
            }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.err2",
            );

            let payloads = h.expect_n_publishes(2).await;
            let resp: PromptResponse = serde_json::from_slice(&payloads[1]).unwrap();
            assert!(
                matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
                "Error in final round must produce EndTurn: {:?}",
                resp.stop_reason
            );
        })
        .await;
}

// ── re-enabled tool reappears in next HTTP call's tool list ───────────────────

#[tokio::test]
async fn reenabled_tool_reappears_in_http_tool_list_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            let cfg_subj = format!("acp.session.{sid}.agent.set_config_option");

            // Disable read_file
            h.session_req(
                &cfg_subj,
                SetSessionConfigOptionRequest::new(sid.clone(), "read_file", "disabled"),
                "r.dis",
            );
            h.expect_n_publishes(2).await;

            // Re-enable read_file
            h.session_req(
                &cfg_subj,
                SetSessionConfigOptionRequest::new(sid.clone(), "read_file", "enabled"),
                "r.enab",
            );
            h.expect_n_publishes(3).await;

            // Prompt — must include read_file in the HTTP call's tool list
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "ok".to_string() }]);
            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.prompt.reen",
            );
            h.expect_n_publishes(4).await;

            let tool_names = h.http.last_tool_names();
            assert!(
                tool_names.iter().any(|n| n == "read_file"),
                "re-enabled read_file must reappear in HTTP call tool list: {tool_names:?}"
            );
        })
        .await;
}

// ── ToolCall / ToolCallUpdate notifications carry correct call_id and title ───

#[tokio::test]
async fn tool_notifications_carry_correct_call_id_and_title_via_nats() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let sid = create_session(&h).await;

            h.http.push(vec![OpenRouterEvent::ToolCallsReady {
                calls: vec![AssembledToolCall {
                    id: "call_verify_id".to_string(),
                    name: "list_directory".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                }],
            }]);
            h.http.push(vec![OpenRouterEvent::TextDelta { text: "done".to_string() }]);

            let prompt_subj = format!("acp.session.{sid}.agent.prompt");
            h.session_req(
                &prompt_subj,
                PromptRequest::new(sid.clone(), vec![ContentBlock::from("q")]),
                "r.ids",
            );
            h.expect_n_publishes(2).await;

            let notes = h.notifier.notifications.lock().unwrap();

            // ToolCall (InProgress): call_id and title must match the dispatched call
            let in_progress = notes.iter().find_map(|n| {
                if let SessionUpdate::ToolCall(tc) = &n.update { Some(tc) } else { None }
            }).expect("must have ToolCall notification");

            assert_eq!(
                in_progress.tool_call_id.0.as_ref(),
                "call_verify_id",
                "ToolCall notification tool_call_id must match dispatched call id"
            );
            assert_eq!(
                in_progress.title,
                "list_directory",
                "ToolCall notification title must match dispatched call name"
            );

            // ToolCallUpdate (Completed): tool_call_id must match the same call
            let completed = notes.iter().find_map(|n| {
                if let SessionUpdate::ToolCallUpdate(tcu) = &n.update { Some(tcu) } else { None }
            }).expect("must have ToolCallUpdate notification");

            assert_eq!(
                completed.tool_call_id.0.as_ref(),
                "call_verify_id",
                "ToolCallUpdate notification tool_call_id must match dispatched call id"
            );
        })
        .await;
}
