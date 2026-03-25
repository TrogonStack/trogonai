//! Integration tests for `RpcServer` handlers.
//!
//! Requires Docker (testcontainers starts a NATS server with JetStream).
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test rpc_server_integration

use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::{
    AuthenticateRequest, AuthenticateResponse, ForkSessionRequest, ForkSessionResponse,
    InitializeRequest, InitializeResponse, ListSessionsRequest, ListSessionsResponse,
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse,
    ProtocolVersion, ResumeSessionRequest, ResumeSessionResponse, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse,
    SetSessionModelRequest, SetSessionModelResponse,
};
use async_nats::jetstream;
use bytes::Bytes;
use futures_util::StreamExt;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
use tokio::sync::RwLock;
use trogon_acp_runner::{RpcServer, SessionState, SessionStore};

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

/// Start an RpcServer, return the NATS client and store for inspection.
async fn start_rpc_server(
    nats: async_nats::Client,
    js: jetstream::Context,
    prefix: &str,
) -> SessionStore {
    let store = SessionStore::open(&js).await.unwrap();
    let store_clone = store.clone();
    let gateway_config = Arc::new(RwLock::new(None));
    let server = RpcServer::new(nats, store_clone, prefix, "claude-opus-4-6", gateway_config);
    tokio::spawn(async move { server.run().await });
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
    let _ = start_rpc_server(nats.clone(), js, "acp").await;

    let req = InitializeRequest::new(ProtocolVersion::LATEST);
    let reply = nats
        .request("acp.agent.initialize", request_bytes(&req))
        .await
        .expect("initialize must reply");

    let resp: InitializeResponse =
        serde_json::from_slice(&reply.payload).expect("reply must be valid JSON");
    assert_eq!(resp.protocol_version, ProtocolVersion::LATEST);

    // Verify key capabilities are advertised.
    let caps = resp.agent_capabilities;
    assert!(caps.load_session, "must advertise load_session");
    let session_caps = caps.session_capabilities;
    assert!(session_caps.list.is_some(), "must advertise session list");
    assert!(session_caps.fork.is_some(), "must advertise session fork");
    assert!(
        session_caps.resume.is_some(),
        "must advertise session resume"
    );
}

// ── authenticate ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn authenticate_returns_empty_response() {
    let (_container, nats, js) = start_nats().await;
    let _ = start_rpc_server(nats.clone(), js, "acp").await;

    let req = AuthenticateRequest::new("password");
    let reply = nats
        .request("acp.agent.authenticate", request_bytes(&req))
        .await
        .expect("authenticate must reply");

    let _resp: AuthenticateResponse =
        serde_json::from_slice(&reply.payload).expect("reply must be valid JSON");
}

// ── new_session ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn new_session_creates_session_in_store_and_replies_with_id() {
    let (_container, nats, js) = start_nats().await;
    let store = start_rpc_server(nats.clone(), js, "acp").await;

    let req = NewSessionRequest::new("/home/user/project");
    let reply = nats
        .request("acp.agent.session.new", request_bytes(&req))
        .await
        .expect("new_session must reply");

    let resp: NewSessionResponse =
        serde_json::from_slice(&reply.payload).expect("reply must be valid JSON");
    let session_id = resp.session_id.to_string();
    assert!(!session_id.is_empty(), "session_id must not be empty");

    // Session must be persisted.
    let state = store.load(&session_id).await.unwrap();
    assert_eq!(state.cwd, "/home/user/project");
    assert_eq!(state.mode, "default");
    assert!(!state.created_at.is_empty());
}

#[tokio::test]
async fn new_session_stores_mode_from_meta() {
    let (_container, nats, js) = start_nats().await;
    let store = start_rpc_server(nats.clone(), js, "acp").await;

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
}

#[tokio::test]
async fn new_session_publishes_session_ready() {
    let (_container, nats, js) = start_nats().await;
    let _ = start_rpc_server(nats.clone(), js, "acp").await;

    // Subscribe to the wildcard session.ready subject before sending the request.
    let mut ready_sub = nats
        .subscribe("acp.*.agent.ext.session.ready")
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
}

#[tokio::test]
async fn new_session_bad_payload_does_not_crash_server() {
    let (_container, nats, js) = start_nats().await;
    let _ = start_rpc_server(nats.clone(), js, "acp").await;

    let _ = nats
        .publish("acp.agent.session.new", Bytes::from_static(b"not json"))
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Server still alive.
    let req = NewSessionRequest::new("/tmp");
    nats.request("acp.agent.session.new", request_bytes(&req))
        .await
        .expect("server must be alive after bad payload");
}

// ── load_session ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn load_session_replies_and_publishes_session_ready() {
    let (_container, nats, js) = start_nats().await;
    let store = start_rpc_server(nats.clone(), js, "acp").await;

    store
        .save("sess-load-1", &SessionState::default())
        .await
        .unwrap();

    let mut ready_sub = nats
        .subscribe("acp.sess-load-1.agent.ext.session.ready")
        .await
        .unwrap();

    let req = LoadSessionRequest::new("sess-load-1", "/tmp");
    let reply = nats
        .request("acp.sess-load-1.agent.session.load", request_bytes(&req))
        .await
        .expect("load_session must reply");

    let _resp: LoadSessionResponse = serde_json::from_slice(&reply.payload).unwrap();

    tokio::time::timeout(Duration::from_secs(2), ready_sub.next())
        .await
        .expect("timed out waiting for session.ready")
        .expect("subscription closed");
}

#[tokio::test]
async fn load_session_bad_payload_does_not_crash_server() {
    let (_container, nats, js) = start_nats().await;
    let _ = start_rpc_server(nats.clone(), js, "acp").await;

    let _ = nats
        .publish(
            "acp.sess-bad.agent.session.load",
            Bytes::from_static(b"not json"),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Server still alive.
    let req = LoadSessionRequest::new("sess-bad", "/tmp");
    nats.request("acp.sess-bad.agent.session.load", request_bytes(&req))
        .await
        .expect("server must be alive after bad payload");
}

// ── set_session_mode ──────────────────────────────────────────────────────────

#[tokio::test]
async fn set_session_mode_updates_mode_in_store() {
    let (_container, nats, js) = start_nats().await;
    let store = start_rpc_server(nats.clone(), js, "acp").await;

    store
        .save("sess-mode-1", &SessionState::default())
        .await
        .unwrap();

    let req = SetSessionModeRequest::new("sess-mode-1", "acceptEdits");
    let reply = nats
        .request(
            "acp.sess-mode-1.agent.session.set_mode",
            request_bytes(&req),
        )
        .await
        .expect("set_session_mode must reply");

    let _resp: SetSessionModeResponse = serde_json::from_slice(&reply.payload).unwrap();

    let state = store.load("sess-mode-1").await.unwrap();
    assert_eq!(state.mode, "acceptEdits");
}

#[tokio::test]
async fn set_session_mode_bad_payload_does_not_crash_server() {
    let (_container, nats, js) = start_nats().await;
    let _ = start_rpc_server(nats.clone(), js, "acp").await;

    let _ = nats
        .publish(
            "acp.sess-bad.agent.session.set_mode",
            Bytes::from_static(b"{{invalid"),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let req = SetSessionModeRequest::new("sess-alive", "default");
    nats.request("acp.sess-alive.agent.session.set_mode", request_bytes(&req))
        .await
        .expect("server must be alive after bad payload");
}

// ── set_session_model ─────────────────────────────────────────────────────────

#[tokio::test]
async fn set_session_model_updates_model_in_store() {
    let (_container, nats, js) = start_nats().await;
    let store = start_rpc_server(nats.clone(), js, "acp").await;

    store
        .save("sess-model-1", &SessionState::default())
        .await
        .unwrap();

    let req = SetSessionModelRequest::new("sess-model-1", "claude-opus-4");
    let reply = nats
        .request(
            "acp.sess-model-1.agent.session.set_model",
            request_bytes(&req),
        )
        .await
        .expect("set_session_model must reply");

    let _resp: SetSessionModelResponse = serde_json::from_slice(&reply.payload).unwrap();

    let state = store.load("sess-model-1").await.unwrap();
    assert_eq!(state.model.as_deref(), Some("claude-opus-4"));
}

#[tokio::test]
async fn set_session_model_works_for_session_that_does_not_exist() {
    let (_container, nats, js) = start_nats().await;
    let store = start_rpc_server(nats.clone(), js, "acp").await;

    let req = SetSessionModelRequest::new("new-sess", "claude-sonnet-4");
    let reply = nats
        .request("acp.new-sess.agent.session.set_model", request_bytes(&req))
        .await
        .expect("must reply even for unknown sessions");

    let _resp: SetSessionModelResponse = serde_json::from_slice(&reply.payload).unwrap();

    let state = store.load("new-sess").await.unwrap();
    assert_eq!(state.model.as_deref(), Some("claude-sonnet-4"));
}

#[tokio::test]
async fn set_session_model_bad_payload_does_not_crash_server() {
    let (_container, nats, js) = start_nats().await;
    let _ = start_rpc_server(nats.clone(), js, "acp").await;

    let _ = nats
        .publish(
            "acp.sess-bad.agent.session.set_model",
            Bytes::from_static(b"not json at all"),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let req = SetSessionModelRequest::new("sess-alive", "claude-haiku-4-5");
    nats.request(
        "acp.sess-alive.agent.session.set_model",
        request_bytes(&req),
    )
    .await
    .expect("server must still be alive after bad payload");
}

// ── list_sessions ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_returns_empty_when_no_sessions() {
    let (_container, nats, js) = start_nats().await;
    let _ = start_rpc_server(nats.clone(), js, "acp").await;

    let req = ListSessionsRequest::new();
    let reply = nats
        .request("acp.agent.session.list", request_bytes(&req))
        .await
        .expect("list_sessions must reply");

    let resp: ListSessionsResponse = serde_json::from_slice(&reply.payload).unwrap();
    assert!(resp.sessions.is_empty());
}

#[tokio::test]
async fn list_sessions_returns_all_saved_sessions_with_metadata() {
    let (_container, nats, js) = start_nats().await;
    let store = start_rpc_server(nats.clone(), js, "acp").await;

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
}

// ── fork_session ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn fork_session_clones_state_under_new_id() {
    let (_container, nats, js) = start_nats().await;
    let store = start_rpc_server(nats.clone(), js, "acp").await;

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
        .request("acp.original.agent.session.fork", request_bytes(&req))
        .await
        .expect("fork_session must reply");

    let resp: ForkSessionResponse = serde_json::from_slice(&reply.payload).unwrap();
    let forked_id = resp.session_id.to_string();
    assert!(!forked_id.is_empty());
    assert_ne!(forked_id, "original", "forked session must have a new ID");

    // Forked session must have the same state as the original.
    let forked = store.load(&forked_id).await.unwrap();
    assert_eq!(forked.cwd, "/home/user/project");
    assert_eq!(forked.mode, "acceptEdits");
    assert!(!forked.created_at.is_empty(), "fork must set created_at");
}

#[tokio::test]
async fn fork_session_bad_payload_does_not_crash_server() {
    let (_container, nats, js) = start_nats().await;
    let _ = start_rpc_server(nats.clone(), js, "acp").await;

    let _ = nats
        .publish(
            "acp.sess-bad.agent.session.fork",
            Bytes::from_static(b"not json"),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Server still alive — send a valid request to confirm.
    let store_check_nats = nats.clone();
    let req = ForkSessionRequest::new("nonexistent", "/tmp");
    store_check_nats
        .request("acp.nonexistent.agent.session.fork", request_bytes(&req))
        .await
        .expect("server must be alive after bad payload");
}

// ── set_session_config_option ─────────────────────────────────────────────────

#[tokio::test]
async fn set_session_config_option_replies_with_empty_updates() {
    let (_container, nats, js) = start_nats().await;
    let _ = start_rpc_server(nats.clone(), js, "acp").await;

    let req = SetSessionConfigOptionRequest::new("sess-cfg-1", "theme", "dark");
    let reply = nats
        .request(
            "acp.sess-cfg-1.agent.session.set_config_option",
            request_bytes(&req),
        )
        .await
        .expect("set_session_config_option must reply");

    let resp: SetSessionConfigOptionResponse =
        serde_json::from_slice(&reply.payload).expect("reply must be valid JSON");
    assert!(
        resp.config_options.is_empty(),
        "response must have no updates"
    );
}

#[tokio::test]
async fn set_session_config_option_bad_payload_does_not_crash_server() {
    let (_container, nats, js) = start_nats().await;
    let _ = start_rpc_server(nats.clone(), js, "acp").await;

    let _ = nats
        .publish(
            "acp.sess-bad.agent.session.set_config_option",
            Bytes::from_static(b"not json"),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let req = SetSessionConfigOptionRequest::new("sess-alive", "key", "val");
    nats.request(
        "acp.sess-alive.agent.session.set_config_option",
        request_bytes(&req),
    )
    .await
    .expect("server must be alive after bad payload");
}

// ── resume_session ────────────────────────────────────────────────────────────

#[tokio::test]
async fn resume_session_replies_successfully() {
    let (_container, nats, js) = start_nats().await;
    let _ = start_rpc_server(nats.clone(), js, "acp").await;

    let req = ResumeSessionRequest::new("sess-resume-1", "/tmp");
    let reply = nats
        .request(
            "acp.sess-resume-1.agent.session.resume",
            request_bytes(&req),
        )
        .await
        .expect("resume_session must reply");

    let _resp: ResumeSessionResponse =
        serde_json::from_slice(&reply.payload).expect("reply must be valid JSON");
}

#[tokio::test]
async fn resume_session_bad_payload_does_not_crash_server() {
    let (_container, nats, js) = start_nats().await;
    let _ = start_rpc_server(nats.clone(), js, "acp").await;

    let _ = nats
        .publish(
            "acp.sess-bad.agent.session.resume",
            Bytes::from_static(b"not json"),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let req = ResumeSessionRequest::new("sess-alive", "/tmp");
    nats.request("acp.sess-alive.agent.session.resume", request_bytes(&req))
        .await
        .expect("server must be alive after bad payload");
}
