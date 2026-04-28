//! End-to-end integration tests: XaiAgent + AgentSideNatsConnection + real NATS.
//!
//! Verifies that XaiAgent correctly handles ACP request-reply over a real NATS
//! server. The xAI HTTP client is replaced with a mock so no real API key is
//! needed. Only `initialize` and `session/new` are exercised — neither touches
//! the xAI API.
//!
//! Requires Docker (testcontainers starts a NATS server).
//!
//! Run with:
//!   cargo test -p trogon-xai-runner --test nats_e2e --features test-helpers

use std::time::Duration;

use acp_nats::AcpPrefix;
use acp_nats_agent::AgentSideNatsConnection;
use serde_json::Value;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_xai_runner::{MockXaiHttpClient, NatsSessionNotifier, XaiAgent};

// ── helpers ───────────────────────────────────────────────────────────────────

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

/// Spawn XaiAgent in a dedicated thread and wait for it to subscribe.
async fn start_agent(nats: async_nats::Client) {
    let nats_for_thread = nats.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        let prefix = AcpPrefix::new("acp").unwrap();
        let notifier = NatsSessionNotifier::new(nats_for_thread.clone(), prefix.clone());
        let agent = XaiAgent::with_deps(notifier, "grok-4", "", MockXaiHttpClient::default());
        let (_, io_task) = AgentSideNatsConnection::new(agent, nats_for_thread, prefix, |fut| {
            tokio::task::spawn_local(fut);
        });
        rt.block_on(local.run_until(async move { io_task.await.ok(); }));
    });
    // Give the agent time to subscribe before the test sends requests.
    tokio::time::sleep(Duration::from_millis(300)).await;
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// XaiAgent handles `initialize` over real NATS and returns capabilities.
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

/// XaiAgent handles `session/new` over real NATS and returns a non-empty session ID.
#[tokio::test]
async fn e2e_nats_session_new_returns_session_id() {
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
