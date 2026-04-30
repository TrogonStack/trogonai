//! End-to-end integration tests: DefaultCodexAgent + AgentSideNatsConnection + real NATS.
//!
//! Verifies that DefaultCodexAgent correctly handles ACP request-reply over a
//! real NATS server. `initialize` is tested without any subprocess. `session/new`
//! requires CODEX_BIN to be set (uses the in-tree mock_codex_server binary).
//!
//! Requires Docker (testcontainers starts a NATS server).
//!
//! Run with:
//!   cargo test -p trogon-codex-runner --test nats_e2e

use std::sync::OnceLock;
use std::time::Duration;

use acp_nats::AcpPrefix;
use acp_nats_agent::AgentSideNatsConnection;
use serde_json::Value;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tokio::sync::Mutex;
use trogon_codex_runner::DefaultCodexAgent;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Path to the compiled mock_codex_server binary (resolved at compile time).
const MOCK_BIN: &str = env!("CARGO_BIN_EXE_mock_codex_server");

/// Serialise CODEX_BIN env-var writes across parallel tests in this binary.
static BIN_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn bin_env_lock() -> &'static Mutex<()> {
    BIN_ENV_LOCK.get_or_init(Mutex::default)
}

async fn start_nats() -> (ContainerAsync<Nats>, async_nats::Client) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    (container, nats)
}

/// Spawn DefaultCodexAgent in a dedicated thread and wait for it to subscribe.
async fn start_agent(nats: async_nats::Client) {
    let nats_for_thread = nats.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        let prefix = AcpPrefix::new("acp").unwrap();
        let agent = DefaultCodexAgent::with_nats(nats_for_thread.clone(), prefix.clone(), "o4-mini");
        let (_, io_task) = AgentSideNatsConnection::new(agent, nats_for_thread, prefix, |fut| {
            tokio::task::spawn_local(fut);
        });
        rt.block_on(local.run_until(async move { io_task.await.ok(); }));
    });
    // Give the agent time to subscribe before the test sends requests.
    tokio::time::sleep(Duration::from_millis(300)).await;
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// DefaultCodexAgent handles `initialize` over real NATS and returns capabilities.
/// No subprocess is spawned — initialize is handled entirely in-process.
#[tokio::test]
async fn e2e_nats_initialize_returns_capabilities() {
    let (_container, nats) = start_nats().await;
    start_agent(nats.clone()).await;

    let payload = serde_json::to_vec(&serde_json::json!({"protocolVersion": 0})).unwrap();
    let msg = tokio::time::timeout(
        Duration::from_secs(10),
        nats.request("acp.agent.initialize", payload.into()),
    )
    .await
    .expect("timed out waiting for initialize response")
    .expect("NATS request failed");

    let response: Value = serde_json::from_slice(&msg.payload).unwrap();
    assert!(
        response["protocolVersion"].is_number(),
        "must have protocolVersion in response: {response}"
    );
    assert!(
        response["agentCapabilities"].is_object(),
        "must have agentCapabilities: {response}"
    );
}

/// DefaultCodexAgent handles `session/new` over real NATS.
/// Spawns the mock_codex_server binary via CODEX_BIN env var.
#[tokio::test]
async fn e2e_nats_session_new_returns_session_id() {
    let _lock = bin_env_lock().lock().await;
    // SAFETY: serialized by BIN_ENV_LOCK; single-process test binary.
    unsafe { std::env::set_var("CODEX_BIN", MOCK_BIN) };

    let (_container, nats) = start_nats().await;
    start_agent(nats.clone()).await;

    let payload = serde_json::to_vec(&serde_json::json!({
        "sessionId": null,
        "cwd": "/tmp",
        "mcpServers": []
    }))
    .unwrap();
    let msg = tokio::time::timeout(
        Duration::from_secs(10),
        nats.request("acp.agent.session.new", payload.into()),
    )
    .await
    .expect("timed out waiting for session/new response")
    .expect("NATS request failed");

    let response: Value = serde_json::from_slice(&msg.payload).unwrap();
    let session_id = response["sessionId"].as_str().unwrap_or("");
    assert!(!session_id.is_empty(), "must have non-empty sessionId: {response}");
}
