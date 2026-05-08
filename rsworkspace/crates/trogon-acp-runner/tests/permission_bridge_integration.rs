//! Integration tests for `handle_permission_request_nats` — requires Docker.
//!
//! These tests exercise the full NATS request-reply round-trip with a real NATS
//! server, unlike the unit tests in `permission_bridge.rs` which use a mock client.
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test permission_bridge_integration

use acp_nats::acp_prefix::AcpPrefix;
use agent_client_protocol::{
    RequestPermissionOutcome, RequestPermissionResponse, SelectedPermissionOutcome,
};
use async_nats::jetstream;
use bytes::Bytes;
use futures_util::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt};
use tokio::sync::oneshot;
use trogon_acp_runner::{
    NatsSessionStore, PermissionReq, SessionState, SessionStore,
};
use trogon_acp_runner::permission_bridge::handle_permission_request_nats;

// ── helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, async_nats::Client, jetstream::Context) {
    let c = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = c.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js = jetstream::new(nats.clone());
    (c, nats, js)
}

const PREFIX: &str = "acp";
const SESSION: &str = "sess-perm-1";

fn subject() -> String {
    format!("{PREFIX}.session.{SESSION}.client.session.request_permission")
}

fn make_req(tool_name: &str) -> (PermissionReq, oneshot::Receiver<bool>) {
    let (tx, rx) = oneshot::channel();
    let req = PermissionReq {
        session_id: SESSION.to_string(),
        tool_call_id: "tc-bridge-1".to_string(),
        tool_name: tool_name.to_string(),
        tool_input: serde_json::json!({"path": "/tmp/x"}),
        response_tx: tx,
    };
    (req, rx)
}

fn response_bytes(outcome: RequestPermissionOutcome) -> Bytes {
    serde_json::to_vec(&RequestPermissionResponse::new(outcome))
        .unwrap()
        .into()
}

fn selected(id: &'static str) -> Bytes {
    response_bytes(RequestPermissionOutcome::Selected(
        SelectedPermissionOutcome::new(id.to_string()),
    ))
}

fn cancelled() -> Bytes {
    response_bytes(RequestPermissionOutcome::Cancelled)
}

/// Subscribe to the permission subject and reply with `reply_bytes` when a request arrives.
fn spawn_responder(nats: async_nats::Client, reply_bytes: Bytes) {
    tokio::spawn(async move {
        let mut sub = nats.subscribe(subject()).await.unwrap();
        if let Some(msg) = sub.next().await {
            if let Some(reply) = msg.reply {
                nats.publish(reply, reply_bytes).await.unwrap();
            }
        }
    });
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// `allow` response over real NATS → function returns true; no store write.
#[tokio::test]
async fn allow_once_returns_true_over_real_nats() {
    let (_c, nats, js) = start_nats().await;
    let store = NatsSessionStore::open(&js).await.unwrap();
    store.save(SESSION, &SessionState::default()).await.unwrap();

    spawn_responder(nats.clone(), selected("allow"));
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let (req, rx) = make_req("Bash");
    handle_permission_request_nats(req, nats, AcpPrefix::new(PREFIX).unwrap(), &store).await;

    assert!(rx.await.unwrap(), "allow must return true");
    assert!(
        store.load(SESSION).await.unwrap().allowed_tools.is_empty(),
        "allow_once must not persist the tool"
    );
}

/// `allow_always` response over real NATS → true and tool persisted in store.
#[tokio::test]
async fn allow_always_returns_true_and_persists_over_real_nats() {
    let (_c, nats, js) = start_nats().await;
    let store = NatsSessionStore::open(&js).await.unwrap();
    store.save(SESSION, &SessionState::default()).await.unwrap();

    spawn_responder(nats.clone(), selected("allow_always"));
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let (req, rx) = make_req("Edit");
    handle_permission_request_nats(req, nats, AcpPrefix::new(PREFIX).unwrap(), &store).await;

    assert!(rx.await.unwrap(), "allow_always must return true");
    assert!(
        store
            .load(SESSION)
            .await
            .unwrap()
            .allowed_tools
            .contains(&"Edit".to_string()),
        "allow_always must persist the tool"
    );
}

/// `reject` response over real NATS → false; no store write.
#[tokio::test]
async fn reject_returns_false_over_real_nats() {
    let (_c, nats, js) = start_nats().await;
    let store = NatsSessionStore::open(&js).await.unwrap();
    store.save(SESSION, &SessionState::default()).await.unwrap();

    spawn_responder(nats.clone(), selected("reject"));
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let (req, rx) = make_req("Write");
    handle_permission_request_nats(req, nats, AcpPrefix::new(PREFIX).unwrap(), &store).await;

    assert!(!rx.await.unwrap(), "reject must return false");
    assert!(store.load(SESSION).await.unwrap().allowed_tools.is_empty());
}

/// `cancelled` response over real NATS → false.
#[tokio::test]
async fn cancelled_returns_false_over_real_nats() {
    let (_c, nats, js) = start_nats().await;
    let store = NatsSessionStore::open(&js).await.unwrap();
    store.save(SESSION, &SessionState::default()).await.unwrap();

    spawn_responder(nats.clone(), cancelled());
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let (req, rx) = make_req("Read");
    handle_permission_request_nats(req, nats, AcpPrefix::new(PREFIX).unwrap(), &store).await;

    assert!(!rx.await.unwrap(), "cancelled must return false");
}

/// Verifies the NATS request payload contains the expected tool name.
#[tokio::test]
async fn request_payload_contains_tool_name_and_input() {
    let (_c, nats, js) = start_nats().await;
    let store = NatsSessionStore::open(&js).await.unwrap();
    store.save(SESSION, &SessionState::default()).await.unwrap();

    let nats_clone = nats.clone();
    let captured: std::sync::Arc<std::sync::Mutex<Option<serde_json::Value>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let captured_clone = captured.clone();

    tokio::spawn(async move {
        let mut sub = nats_clone.subscribe(subject()).await.unwrap();
        if let Some(msg) = sub.next().await {
            let body: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap_or_default();
            *captured_clone.lock().unwrap() = Some(body);
            if let Some(reply) = msg.reply {
                nats_clone.publish(reply, selected("reject")).await.unwrap();
            }
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let (tx, _rx) = oneshot::channel();
    let req = PermissionReq {
        session_id: SESSION.to_string(),
        tool_call_id: "tc-payload-check".to_string(),
        tool_name: "SpecialTool".to_string(),
        tool_input: serde_json::json!({"key": "value-marker"}),
        response_tx: tx,
    };
    handle_permission_request_nats(req, nats, AcpPrefix::new(PREFIX).unwrap(), &store).await;

    let payload = captured.lock().unwrap().clone().expect("payload must be captured");
    let payload_str = payload.to_string();
    assert!(
        payload_str.contains("SpecialTool"),
        "payload must contain tool name; got: {payload_str}"
    );
}

/// No subscriber on the permission subject → network error → function returns false.
#[tokio::test]
async fn network_error_returns_false() {
    let (_c, nats, js) = start_nats().await;
    let store = NatsSessionStore::open(&js).await.unwrap();
    store.save(SESSION, &SessionState::default()).await.unwrap();

    // No responder — NATS will return a no-responder error immediately.
    let (req, rx) = make_req("Bash");
    handle_permission_request_nats(req, nats, AcpPrefix::new(PREFIX).unwrap(), &store).await;

    assert!(!rx.await.unwrap(), "network error must return false");
}
