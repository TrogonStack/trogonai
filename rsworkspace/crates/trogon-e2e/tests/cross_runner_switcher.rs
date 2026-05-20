//! Integration test for `CrossRunnerSwitcher` against two real `TrogonAgent` instances.
//!
//! Spins up a NATS JetStream server (testcontainers), starts two TrogonAgent
//! instances on different ACP prefixes ("acp.src" and "acp.dst"), seeds the
//! source session with known messages, then drives `CrossRunnerSwitcher::switch_model`
//! end-to-end through real NATS request-reply and verifies the migrated session.
//!
//! Requires Docker.

use std::sync::Arc;
use std::time::Duration;

use acp_nats::{AcpPrefix, Config};
use acp_nats_agent::AgentSideNatsConnection;
use async_nats::jetstream;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tokio::sync::RwLock;
use trogon_acp_runner::{
    GatewayConfig, NatsSessionStore, SessionState, SessionStore,
    TrogonAgent,
    agent_runner::mock::MockAgentRunner,
    session_notifier::mock::MockSessionNotifier,
};
use trogon_agent_core::agent_loop::{ContentBlock as AgentContentBlock, Message as AgentMessage};
use trogon_cli::CrossRunnerSwitcher;
use trogon_nats::{NatsAuth, NatsConfig};
use trogon_registry::{AgentCapability, MockRegistryStore, Registry};

// ── helpers ───────────────────────────────────────────────────────────────────

async fn start_nats_js() -> (ContainerAsync<Nats>, u16) {
    let c = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = c.get_host_port_ipv4(4222).await.unwrap();
    (c, port)
}

async fn make_nats(port: u16) -> (async_nats::Client, jetstream::Context) {
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    (nats, js)
}

type TestAgent = TrogonAgent<NatsSessionStore, MockAgentRunner, MockSessionNotifier>;

fn make_agent(store: NatsSessionStore, prefix: &str) -> TestAgent {
    TrogonAgent::new(
        MockSessionNotifier::new(),
        store,
        MockAgentRunner::new("test-model"),
        prefix,
        "test-model",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    )
}

/// Attach a `TrogonAgent` to NATS via `AgentSideNatsConnection` and spawn its
/// I/O task as a local task. Must be called from within a `LocalSet`.
fn attach_agent(agent: TestAgent, nats: async_nats::Client, prefix: &str) {
    let acp_prefix = AcpPrefix::new(prefix).expect("prefix must be valid");
    let (_, io_task) = AgentSideNatsConnection::new(agent, nats, acp_prefix, |fut| {
        tokio::task::spawn_local(fut);
    });
    tokio::task::spawn_local(async move {
        io_task.await.ok();
    });
}

fn make_config(port: u16) -> Config {
    Config::new(
        AcpPrefix::new("acp.src").unwrap(),
        NatsConfig {
            servers: vec![format!("127.0.0.1:{port}")],
            auth: NatsAuth::None,
        },
    )
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// `CrossRunnerSwitcher::switch_model` migrates a session from one real
/// TrogonAgent to another over actual NATS JetStream.
///
/// The test seeds messages directly into the source KV store, drives the
/// switcher end-to-end (export→new_session→import over NATS), then reads the
/// destination session from the shared KV bucket to verify the messages arrived.
#[tokio::test]
async fn switch_model_migrates_history_between_two_acp_runners() {
    let (_c, port) = start_nats_js().await;
    let (nats, js) = make_nats(port).await;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            // ── 1. Open shared KV store and seed source session ───────────────
            let store = NatsSessionStore::open(&js).await.unwrap();

            let src_state = SessionState {
                messages: vec![
                    AgentMessage::user_text("original question"),
                    AgentMessage::assistant(vec![AgentContentBlock::Text {
                        text: "original answer".into(),
                    }]),
                ],
                ..Default::default()
            };
            store.save("switcher-src-1", &src_state).await.unwrap();

            // ── 2. Start two TrogonAgent instances on different prefixes ───────
            attach_agent(make_agent(store.clone(), "acp.src"), nats.clone(), "acp.src");
            attach_agent(make_agent(store.clone(), "acp.dst"), nats.clone(), "acp.dst");

            // Allow subscriptions to settle before sending requests.
            tokio::time::sleep(Duration::from_millis(60)).await;

            // ── 3. Build CrossRunnerSwitcher: "dst-model" → "acp.dst" ─────────
            let registry = Registry::new(MockRegistryStore::new());
            let mut dst_cap =
                AgentCapability::new("dst-runner", ["chat"], "agents.dst.>");
            dst_cap.metadata =
                serde_json::json!({ "models": ["dst-model"], "acp_prefix": "acp.dst" });
            registry.register(&dst_cap).await.unwrap();

            let mut switcher =
                CrossRunnerSwitcher::new(nats.clone(), make_config(port), registry);

            // ── 4. Migrate the session ────────────────────────────────────────
            let (new_prefix, new_session_id) = switcher
                .switch_model("acp.src", "switcher-src-1", "dst-model", "/tmp")
                .await
                .expect("switch_model should succeed");

            assert_eq!(new_prefix, "acp.dst");
            assert!(!new_session_id.is_empty());

            // ── 5. Verify migrated messages in dst via shared KV bucket ───────
            let dst_state = store.load(&new_session_id).await.unwrap();

            assert_eq!(
                dst_state.messages.len(),
                2,
                "migrated session must have 2 messages"
            );
            assert_eq!(
                dst_state.messages[0].role, "user",
                "first migrated message must be user"
            );
            assert_eq!(
                dst_state.messages[1].role, "assistant",
                "second migrated message must be assistant"
            );
            assert!(
                matches!(
                    &dst_state.messages[0].content[0],
                    AgentContentBlock::Text { text } if text == "original question"
                ),
                "user message text must match after migration"
            );
            assert!(
                matches!(
                    &dst_state.messages[1].content[0],
                    AgentContentBlock::Text { text } if text == "original answer"
                ),
                "assistant message text must match after migration"
            );
        })
        .await;
}
