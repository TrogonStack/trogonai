//! End-to-end integration tests: OpenRouterAgent + AgentSideNatsConnection + real NATS.
//!
//! Verifies that OpenRouterAgent correctly handles ACP request-reply over a real NATS
//! server. The OpenRouter HTTP client is replaced with a no-op stub so no real API key
//! is needed. Only `initialize` and `session/new` are exercised — neither touches
//! the OpenRouter API.
//!
//! Requires Docker (testcontainers starts a NATS server).
//!
//! Run with:
//!   cargo test -p trogon-openrouter-runner --test nats_e2e

use std::time::Duration;

use acp_nats::AcpPrefix;
use acp_nats_agent::AgentSideNatsConnection;
use async_trait::async_trait;
use futures_util::stream::{self, LocalBoxStream};
use serde_json::Value;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_openrouter_runner::{
    Message, NatsSessionNotifier, OpenRouterAgent, OpenRouterEvent, OpenRouterHttpClient,
};

// ── No-op HTTP client stub ────────────────────────────────────────────────────

struct NoOpHttpClient;

#[async_trait(?Send)]
impl OpenRouterHttpClient for NoOpHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _messages: &[Message],
        _api_key: &str,
    ) -> LocalBoxStream<'static, OpenRouterEvent> {
        Box::pin(stream::empty())
    }
}

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
        let agent = OpenRouterAgent::with_deps(notifier, "test-model", "", NoOpHttpClient);
        let (_, io_task) = AgentSideNatsConnection::new(agent, nats_for_thread, prefix, |fut| {
            tokio::task::spawn_local(fut);
        });
        rt.block_on(local.run_until(async move { io_task.await.ok(); }));
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
}

// ── tests ─────────────────────────────────────────────────────────────────────

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

#[tokio::test]
async fn openrouter_runner_registers_with_correct_acp_prefix_metadata() {
    let (_container, nats) = start_nats().await;
    let js = async_nats::jetstream::new(nats.clone());

    let prefix = "acp.openrouter";
    let agent_type = "openrouter";

    let store = trogon_registry::provision(&js).await.expect("provision registry");
    let registry = trogon_registry::Registry::new(store);

    let cap = trogon_registry::AgentCapability {
        agent_type: agent_type.to_string(),
        capabilities: vec!["chat".to_string()],
        nats_subject: format!("{}.agent.>", prefix),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": prefix }),
    };
    registry.register(&cap).await.expect("registration must succeed");

    let entry = registry
        .get(agent_type)
        .await
        .expect("get must not error")
        .expect("registered entry must exist");

    assert_eq!(
        entry.metadata["acp_prefix"].as_str(),
        Some(prefix),
        "bridge relies on acp_prefix matching ACP_PREFIX — got {:?}",
        entry.metadata
    );
    assert_eq!(
        entry.nats_subject,
        format!("{}.agent.>", prefix),
        "nats_subject must be derived from ACP_PREFIX"
    );
}
