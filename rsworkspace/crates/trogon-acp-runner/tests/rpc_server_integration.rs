//! Integration tests for `TrogonAgent` RPC handlers (all ACP methods except prompt).
//!
//! Requires Docker (testcontainers starts a NATS server with JetStream).
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test rpc_server_integration

use std::sync::Arc;
use std::time::Duration;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats_agent::AgentSideNatsConnection;
use agent_client_protocol::{
    AuthenticateRequest, AuthenticateResponse, CloseSessionRequest, CloseSessionResponse,
    ForkSessionRequest, ForkSessionResponse, InitializeRequest, InitializeResponse,
    ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    NewSessionRequest, NewSessionResponse, ProtocolVersion, ResumeSessionRequest,
    ResumeSessionResponse, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
    SetSessionModeRequest, SetSessionModeResponse, SetSessionModelRequest, SetSessionModelResponse,
};
use async_nats::jetstream;
use bytes::Bytes;
use futures_util::StreamExt;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
use tokio::sync::RwLock;
use trogon_acp_runner::{NatsSessionNotifier, NatsSessionStore, SessionState, SessionStore, TrogonAgent};
use trogon_agent_core::agent_loop::AgentLoop;
use trogon_agent_core::tools::ToolContext;

// ── helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, async_nats::Client, jetstream::Context) {
    let container: ContainerAsync<Nats> = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    (container, nats, js)
}

fn make_agent_loop() -> AgentLoop {
    let http = reqwest::Client::new();
    AgentLoop {
        http_client: http.clone(),
        proxy_url: "http://127.0.0.1:1".to_string(),
        anthropic_token: "test-token".to_string(),
        anthropic_base_url: Some("http://127.0.0.1:1".to_string()),
        anthropic_extra_headers: vec![],
        streaming_client: None,
        model: "claude-test".to_string(),
        max_iterations: 1,
        thinking_budget: None,
        tool_context: Arc::new(ToolContext {
            proxy_url: "http://127.0.0.1:1".to_string(),
        }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        permission_checker: None,
        elicitation_provider: None,
    }
}

/// Start a `TrogonAgent` via `AgentSideNatsConnection`, return the `NatsSessionStore`.
///
/// Must be called from within a `LocalSet` (uses `spawn_local` internally).
async fn start_agent(
    nats: async_nats::Client,
    js: &jetstream::Context,
    prefix: &str,
) -> NatsSessionStore {
    let store = NatsSessionStore::open(js).await.unwrap();
    let notifier = NatsSessionNotifier::new(nats.clone());
    let gateway_config = Arc::new(RwLock::new(None));
    let agent = TrogonAgent::new(
        notifier,
        store.clone(),
        make_agent_loop(),
        prefix,
        "claude-opus-4-6",
        None,
        None,
        gateway_config,
    );
    let acp_prefix = AcpPrefix::new(prefix).unwrap();
    let (_, io_task) = AgentSideNatsConnection::new(agent, nats, acp_prefix, |fut| {
        tokio::task::spawn_local(fut);
    });
    tokio::task::spawn_local(io_task);
    tokio::time::sleep(Duration::from_millis(50)).await;
    store
}

fn request_bytes<T: serde::Serialize>(v: &T) -> Bytes {
    Bytes::from(serde_json::to_vec(v).unwrap())
}

// ── initialize ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn initialize_returns_protocol_version_and_capabilities() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let _ = start_agent(nats.clone(), &js, "acp").await;

            let req = InitializeRequest::new(ProtocolVersion::LATEST);
            let reply = nats
                .request("acp.agent.initialize", request_bytes(&req))
                .await
                .expect("initialize must reply");

            let resp: InitializeResponse =
                serde_json::from_slice(&reply.payload).expect("reply must be valid JSON");
            assert_eq!(resp.protocol_version, ProtocolVersion::LATEST);

            let caps = resp.agent_capabilities;
            assert!(caps.load_session, "must advertise load_session");
            let session_caps = caps.session_capabilities;
            assert!(session_caps.list.is_some(), "must advertise session list");
            assert!(session_caps.fork.is_some(), "must advertise session fork");
            assert!(session_caps.resume.is_some(), "must advertise session resume");
            let meta = session_caps
                .meta
                .expect("session_capabilities must have _meta");
            assert!(
                meta.get("close").is_some(),
                "session_capabilities._meta must contain 'close'"
            );
        })
        .await;
}

// ── authenticate ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn authenticate_returns_empty_response() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let _ = start_agent(nats.clone(), &js, "acp").await;

            let req = AuthenticateRequest::new("password");
            let reply = nats
                .request("acp.agent.authenticate", request_bytes(&req))
                .await
                .expect("authenticate must reply");

            let _resp: AuthenticateResponse =
                serde_json::from_slice(&reply.payload).expect("reply must be valid JSON");
        })
        .await;
}

// ── new_session ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn new_session_creates_session_in_store_and_replies_with_id() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let store = start_agent(nats.clone(), &js, "acp").await;

            let req = NewSessionRequest::new("/home/user/project");
            let reply = nats
                .request("acp.agent.session.new", request_bytes(&req))
                .await
                .expect("new_session must reply");

            let resp: NewSessionResponse =
                serde_json::from_slice(&reply.payload).expect("reply must be valid JSON");
            let session_id = resp.session_id.to_string();
            assert!(!session_id.is_empty(), "session_id must not be empty");

            let state = store.load(&session_id).await.unwrap();
            assert_eq!(state.cwd, "/home/user/project");
            assert_eq!(state.mode, "default");
            assert!(!state.created_at.is_empty());
        })
        .await;
}

#[tokio::test]
async fn new_session_stores_mode_from_meta() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let store = start_agent(nats.clone(), &js, "acp").await;

            let mut meta = serde_json::Map::new();
            meta.insert(
                "mode".to_string(),
                serde_json::Value::String("bypassPermissions".to_string()),
            );
            let req = NewSessionRequest::new("/tmp").meta(meta);
            let reply = nats
                .request("acp.agent.session.new", request_bytes(&req))
                .await
                .unwrap();

            let resp: NewSessionResponse = serde_json::from_slice(&reply.payload).unwrap();
            let state = store.load(&resp.session_id.to_string()).await.unwrap();
            assert_eq!(state.mode, "bypassPermissions");
        })
        .await;
}

#[tokio::test]
async fn new_session_publishes_session_ready() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let _ = start_agent(nats.clone(), &js, "acp").await;

            let mut ready_sub = nats
                .subscribe("acp.session.*.agent.ext.ready")
                .await
                .unwrap();

            let req = NewSessionRequest::new("/tmp");
            let reply = nats
                .request("acp.agent.session.new", request_bytes(&req))
                .await
                .unwrap();
            let resp: NewSessionResponse = serde_json::from_slice(&reply.payload).unwrap();
            let session_id = resp.session_id.to_string();

            let ready_msg = tokio::time::timeout(Duration::from_secs(2), ready_sub.next())
                .await
                .expect("timed out waiting for session.ready")
                .expect("subscription closed");

            assert!(
                ready_msg.subject.contains(&session_id),
                "session.ready subject must contain the new session_id, got: {}",
                ready_msg.subject
            );
        })
        .await;
}

#[tokio::test]
async fn new_session_bad_payload_does_not_crash_server() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let _ = start_agent(nats.clone(), &js, "acp").await;

            let _ = nats
                .publish("acp.agent.session.new", Bytes::from_static(b"not json"))
                .await;
            tokio::time::sleep(Duration::from_millis(50)).await;

            let req = NewSessionRequest::new("/tmp");
            nats.request("acp.agent.session.new", request_bytes(&req))
                .await
                .expect("server must be alive after bad payload");
        })
        .await;
}

// ── load_session ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn load_session_replies_and_publishes_session_ready() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let store = start_agent(nats.clone(), &js, "acp").await;

            store
                .save("sess-load-1", &SessionState::default())
                .await
                .unwrap();

            let mut ready_sub = nats
                .subscribe("acp.session.sess-load-1.agent.ext.ready")
                .await
                .unwrap();

            let req = LoadSessionRequest::new("sess-load-1", "/tmp");
            let reply = nats
                .request("acp.session.sess-load-1.agent.load", request_bytes(&req))
                .await
                .expect("load_session must reply");

            let _resp: LoadSessionResponse = serde_json::from_slice(&reply.payload).unwrap();

            tokio::time::timeout(Duration::from_secs(2), ready_sub.next())
                .await
                .expect("timed out waiting for session.ready")
                .expect("subscription closed");
        })
        .await;
}

#[tokio::test]
async fn load_session_bad_payload_does_not_crash_server() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let _ = start_agent(nats.clone(), &js, "acp").await;

            let _ = nats
                .publish(
                    "acp.session.sess-bad.agent.load",
                    Bytes::from_static(b"not json"),
                )
                .await;
            tokio::time::sleep(Duration::from_millis(50)).await;

            let req = LoadSessionRequest::new("sess-bad", "/tmp");
            nats.request("acp.session.sess-bad.agent.load", request_bytes(&req))
                .await
                .expect("server must be alive after bad payload");
        })
        .await;
}

// ── set_session_mode ──────────────────────────────────────────────────────────

#[tokio::test]
async fn set_session_mode_updates_mode_in_store() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let store = start_agent(nats.clone(), &js, "acp").await;

            store
                .save("sess-mode-1", &SessionState::default())
                .await
                .unwrap();

            let req = SetSessionModeRequest::new("sess-mode-1", "acceptEdits");
            let reply = nats
                .request(
                    "acp.session.sess-mode-1.agent.set_mode",
                    request_bytes(&req),
                )
                .await
                .expect("set_session_mode must reply");

            let _resp: SetSessionModeResponse = serde_json::from_slice(&reply.payload).unwrap();

            let state = store.load("sess-mode-1").await.unwrap();
            assert_eq!(state.mode, "acceptEdits");
        })
        .await;
}

#[tokio::test]
async fn set_session_mode_bad_payload_does_not_crash_server() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let _ = start_agent(nats.clone(), &js, "acp").await;

            let _ = nats
                .publish(
                    "acp.session.sess-bad.agent.set_mode",
                    Bytes::from_static(b"{{invalid"),
                )
                .await;
            tokio::time::sleep(Duration::from_millis(50)).await;

            let req = SetSessionModeRequest::new("sess-alive", "default");
            nats.request("acp.session.sess-alive.agent.set_mode", request_bytes(&req))
                .await
                .expect("server must be alive after bad payload");
        })
        .await;
}

// ── set_session_model ─────────────────────────────────────────────────────────

#[tokio::test]
async fn set_session_model_updates_model_in_store() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let store = start_agent(nats.clone(), &js, "acp").await;

            store
                .save("sess-model-1", &SessionState::default())
                .await
                .unwrap();

            let req = SetSessionModelRequest::new("sess-model-1", "claude-opus-4");
            let reply = nats
                .request(
                    "acp.session.sess-model-1.agent.set_model",
                    request_bytes(&req),
                )
                .await
                .expect("set_session_model must reply");

            let _resp: SetSessionModelResponse = serde_json::from_slice(&reply.payload).unwrap();

            let state = store.load("sess-model-1").await.unwrap();
            assert_eq!(state.model.as_deref(), Some("claude-opus-4"));
        })
        .await;
}

#[tokio::test]
async fn set_session_model_works_for_session_that_does_not_exist() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let store = start_agent(nats.clone(), &js, "acp").await;

            let req = SetSessionModelRequest::new("new-sess", "claude-sonnet-4");
            let reply = nats
                .request("acp.session.new-sess.agent.set_model", request_bytes(&req))
                .await
                .expect("must reply even for unknown sessions");

            let _resp: SetSessionModelResponse = serde_json::from_slice(&reply.payload).unwrap();

            let state = store.load("new-sess").await.unwrap();
            assert_eq!(state.model.as_deref(), Some("claude-sonnet-4"));
        })
        .await;
}

#[tokio::test]
async fn set_session_model_bad_payload_does_not_crash_server() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let _ = start_agent(nats.clone(), &js, "acp").await;

            let _ = nats
                .publish(
                    "acp.session.sess-bad.agent.set_model",
                    Bytes::from_static(b"not json at all"),
                )
                .await;
            tokio::time::sleep(Duration::from_millis(50)).await;

            let req = SetSessionModelRequest::new("sess-alive", "claude-haiku-4-5");
            nats.request(
                "acp.session.sess-alive.agent.set_model",
                request_bytes(&req),
            )
            .await
            .expect("server must still be alive after bad payload");
        })
        .await;
}

// ── list_sessions ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_returns_empty_when_no_sessions() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let _ = start_agent(nats.clone(), &js, "acp").await;

            let req = ListSessionsRequest::new();
            let reply = nats
                .request("acp.agent.session.list", request_bytes(&req))
                .await
                .expect("list_sessions must reply");

            let resp: ListSessionsResponse = serde_json::from_slice(&reply.payload).unwrap();
            assert!(resp.sessions.is_empty());
        })
        .await;
}

#[tokio::test]
async fn list_sessions_returns_all_saved_sessions_with_metadata() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let store = start_agent(nats.clone(), &js, "acp").await;

            store
                .save(
                    "s1",
                    &SessionState {
                        cwd: "/home/user/proj1".to_string(),
                        title: "First session".to_string(),
                        updated_at: "2026-01-01T00:00:00Z".to_string(),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();

            store
                .save(
                    "s2",
                    &SessionState {
                        cwd: "/home/user/proj2".to_string(),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();

            let req = ListSessionsRequest::new();
            let reply = nats
                .request("acp.agent.session.list", request_bytes(&req))
                .await
                .unwrap();

            let resp: ListSessionsResponse = serde_json::from_slice(&reply.payload).unwrap();
            assert_eq!(resp.sessions.len(), 2);

            let s1 = resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == "s1");
            let s2 = resp
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == "s2");

            assert!(s1.is_some(), "s1 must be in list");
            assert!(s2.is_some(), "s2 must be in list");

            let s1 = s1.unwrap();
            assert_eq!(s1.cwd.to_string_lossy(), "/home/user/proj1");
            assert_eq!(s1.title.as_deref(), Some("First session"));
            assert_eq!(s1.updated_at.as_deref(), Some("2026-01-01T00:00:00Z"));
        })
        .await;
}

// ── fork_session ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn fork_session_clones_state_under_new_id() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let store = start_agent(nats.clone(), &js, "acp").await;

            store
                .save(
                    "original",
                    &SessionState {
                        cwd: "/home/user/project".to_string(),
                        mode: "acceptEdits".to_string(),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();

            let req = ForkSessionRequest::new("original", "/home/user/project");
            let reply = nats
                .request("acp.session.original.agent.fork", request_bytes(&req))
                .await
                .expect("fork_session must reply");

            let resp: ForkSessionResponse = serde_json::from_slice(&reply.payload).unwrap();
            let forked_id = resp.session_id.to_string();
            assert!(!forked_id.is_empty());
            assert_ne!(forked_id, "original");

            let forked = store.load(&forked_id).await.unwrap();
            assert_eq!(forked.cwd, "/home/user/project");
            assert_eq!(forked.mode, "acceptEdits");
            assert!(!forked.created_at.is_empty());
        })
        .await;
}

#[tokio::test]
async fn fork_session_bad_payload_does_not_crash_server() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let _ = start_agent(nats.clone(), &js, "acp").await;

            let _ = nats
                .publish(
                    "acp.session.sess-bad.agent.fork",
                    Bytes::from_static(b"not json"),
                )
                .await;
            tokio::time::sleep(Duration::from_millis(50)).await;

            let req = ForkSessionRequest::new("nonexistent", "/tmp");
            nats.request("acp.session.nonexistent.agent.fork", request_bytes(&req))
                .await
                .expect("server must be alive after bad payload");
        })
        .await;
}

// ── set_session_config_option ─────────────────────────────────────────────────

#[tokio::test]
async fn set_session_config_option_returns_populated_config_options() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let _ = start_agent(nats.clone(), &js, "acp").await;

            let req = SetSessionConfigOptionRequest::new("sess-cfg-1", "mode", "plan");
            let reply = nats
                .request(
                    "acp.session.sess-cfg-1.agent.set_config_option",
                    request_bytes(&req),
                )
                .await
                .expect("set_session_config_option must reply");

            let resp: SetSessionConfigOptionResponse =
                serde_json::from_slice(&reply.payload).expect("reply must be valid JSON");
            assert!(!resp.config_options.is_empty());
            let ids: Vec<String> = resp
                .config_options
                .iter()
                .map(|o| o.id.to_string())
                .collect();
            assert!(ids.iter().any(|id| id == "mode"));
            assert!(ids.iter().any(|id| id == "model"));
        })
        .await;
}

#[tokio::test]
async fn set_session_config_option_mode_updates_store() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let store = start_agent(nats.clone(), &js, "acp").await;

            let new_req = NewSessionRequest::new("/tmp");
            let reply = nats
                .request("acp.agent.session.new", request_bytes(&new_req))
                .await
                .unwrap();
            let new_resp: NewSessionResponse = serde_json::from_slice(&reply.payload).unwrap();
            let session_id = new_resp.session_id.to_string();

            let cfg_req = SetSessionConfigOptionRequest::new(session_id.clone(), "mode", "plan");
            let subject = format!("acp.session.{}.agent.set_config_option", session_id);
            nats.request(subject, request_bytes(&cfg_req))
                .await
                .expect("set_session_config_option must reply");

            let state = store.load(&session_id).await.unwrap();
            assert_eq!(state.mode, "plan");
        })
        .await;
}

#[tokio::test]
async fn set_session_config_option_model_updates_store() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let store = start_agent(nats.clone(), &js, "acp").await;

            let new_req = NewSessionRequest::new("/tmp");
            let reply = nats
                .request("acp.agent.session.new", request_bytes(&new_req))
                .await
                .unwrap();
            let new_resp: NewSessionResponse = serde_json::from_slice(&reply.payload).unwrap();
            let session_id = new_resp.session_id.to_string();

            let cfg_req = SetSessionConfigOptionRequest::new(
                session_id.clone(),
                "model",
                "claude-haiku-4-5-20251001",
            );
            let subject = format!("acp.session.{}.agent.set_config_option", session_id);
            nats.request(subject, request_bytes(&cfg_req))
                .await
                .expect("set_session_config_option must reply");

            let state = store.load(&session_id).await.unwrap();
            assert_eq!(state.model.as_deref(), Some("claude-haiku-4-5-20251001"));
        })
        .await;
}

#[tokio::test]
async fn set_session_config_option_bad_payload_does_not_crash_server() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let _ = start_agent(nats.clone(), &js, "acp").await;

            let _ = nats
                .publish(
                    "acp.session.sess-bad.agent.set_config_option",
                    Bytes::from_static(b"not json"),
                )
                .await;
            tokio::time::sleep(Duration::from_millis(50)).await;

            let req = SetSessionConfigOptionRequest::new("sess-alive", "key", "val");
            nats.request(
                "acp.session.sess-alive.agent.set_config_option",
                request_bytes(&req),
            )
            .await
            .expect("server must be alive after bad payload");
        })
        .await;
}

// ── resume_session ────────────────────────────────────────────────────────────

#[tokio::test]
async fn resume_session_replies_successfully() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let _ = start_agent(nats.clone(), &js, "acp").await;

            let req = ResumeSessionRequest::new("sess-resume-1", "/tmp");
            let reply = nats
                .request(
                    "acp.session.sess-resume-1.agent.resume",
                    request_bytes(&req),
                )
                .await
                .expect("resume_session must reply");

            let _resp: ResumeSessionResponse =
                serde_json::from_slice(&reply.payload).expect("reply must be valid JSON");
        })
        .await;
}

#[tokio::test]
async fn resume_session_bad_payload_does_not_crash_server() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let _ = start_agent(nats.clone(), &js, "acp").await;

            let _ = nats
                .publish(
                    "acp.session.sess-bad.agent.resume",
                    Bytes::from_static(b"not json"),
                )
                .await;
            tokio::time::sleep(Duration::from_millis(50)).await;

            let req = ResumeSessionRequest::new("sess-alive", "/tmp");
            nats.request("acp.session.sess-alive.agent.resume", request_bytes(&req))
                .await
                .expect("server must be alive after bad payload");
        })
        .await;
}

// ── close_session ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn close_session_removes_session_from_store() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let store = start_agent(nats.clone(), &js, "acp").await;

            let new_req = NewSessionRequest::new("/tmp");
            let reply = nats
                .request("acp.agent.session.new", request_bytes(&new_req))
                .await
                .expect("new_session must reply");
            let new_resp: NewSessionResponse =
                serde_json::from_slice(&reply.payload).expect("valid JSON");
            let session_id = new_resp.session_id.to_string();

            let before = store.load(&session_id).await.unwrap();
            assert!(!before.cwd.is_empty());

            let close_req = CloseSessionRequest::new(session_id.clone());
            let reply = nats
                .request(
                    format!("acp.session.{session_id}.agent.close"),
                    request_bytes(&close_req),
                )
                .await
                .expect("close_session must reply");
            let _resp: CloseSessionResponse =
                serde_json::from_slice(&reply.payload).expect("reply must be valid JSON");

            let after = store.load(&session_id).await.unwrap();
            assert!(after.cwd.is_empty());
        })
        .await;
}

#[tokio::test]
async fn close_session_bad_payload_does_not_crash_server() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let _ = start_agent(nats.clone(), &js, "acp").await;

            let _ = nats
                .publish(
                    "acp.session.sess-bad.agent.close",
                    Bytes::from_static(b"not json"),
                )
                .await;
            tokio::time::sleep(Duration::from_millis(50)).await;

            let req = ResumeSessionRequest::new("sess-alive", "/tmp");
            nats.request("acp.session.sess-alive.agent.resume", request_bytes(&req))
                .await
                .expect("server must be alive after bad payload");
        })
        .await;
}

#[tokio::test]
async fn close_session_publishes_cancel_signal() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let store = start_agent(nats.clone(), &js, "acp").await;

            let new_req = NewSessionRequest::new("/tmp");
            let reply = nats
                .request("acp.agent.session.new", request_bytes(&new_req))
                .await
                .expect("new_session must reply");
            let new_resp: NewSessionResponse =
                serde_json::from_slice(&reply.payload).expect("valid JSON");
            let session_id = new_resp.session_id.to_string();

            let cancel_subject = format!("acp.session.{session_id}.agent.cancel");
            let mut cancel_sub = nats
                .subscribe(cancel_subject)
                .await
                .expect("must subscribe");

            let close_req = CloseSessionRequest::new(session_id.clone());
            nats.request(
                format!("acp.session.{session_id}.agent.close"),
                request_bytes(&close_req),
            )
            .await
            .expect("close_session must reply");

            let cancel_msg = tokio::time::timeout(Duration::from_millis(500), cancel_sub.next())
                .await
                .expect("cancel signal must arrive within 500ms")
                .expect("cancel subscription must not be closed");
            drop(cancel_msg);

            let _ = store;
        })
        .await;
}

#[tokio::test]
async fn close_session_nonexistent_session_does_not_crash() {
    let (_container, nats, js) = start_nats().await;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let _ = start_agent(nats.clone(), &js, "acp").await;

            let close_req = CloseSessionRequest::new("nonexistent-session-id");
            let reply = nats
                .request(
                    "acp.session.nonexistent-session-id.agent.close",
                    request_bytes(&close_req),
                )
                .await
                .expect("close_session must reply even for nonexistent session");
            let _resp: CloseSessionResponse =
                serde_json::from_slice(&reply.payload).expect("reply must be valid JSON");
        })
        .await;
}
