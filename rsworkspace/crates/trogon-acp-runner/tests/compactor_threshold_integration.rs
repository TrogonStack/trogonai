//! Integration tests for the 85 % compactor threshold.
//!
//! Verifies that `TrogonAgent::prompt()` calls `trogon.compactor.compact` via
//! NATS request-reply when the session's token estimate exceeds 85 % of
//! `token_budget`, and skips compaction when below threshold.
//!
//! Requires Docker (uses testcontainers to spin up a NATS server).
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test compactor_threshold_integration --features test-helpers

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use agent_client_protocol::{Agent, ContentBlock, NewSessionRequest, PromptRequest, TextContent};
use futures::StreamExt;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use tokio::sync::RwLock;
use trogon_acp_runner::{
    GatewayConfig, TrogonAgent,
    agent_runner::mock::MockAgentRunner,
    session_notifier::mock::MockSessionNotifier,
    session_store::{SessionStore, mock::MemorySessionStore},
};
use trogon_agent_core::agent_loop::Message;

async fn start_nats() -> (testcontainers_modules::testcontainers::ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

async fn nats_client(port: u16) -> async_nats::Client {
    async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("Failed to connect to NATS")
}

/// Compact is called when messages push the estimate over 85 % of token_budget.
#[tokio::test]
async fn compactor_called_when_messages_exceed_85_percent_of_token_budget() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;

    let was_called = Arc::new(AtomicBool::new(false));

    let store = MemorySessionStore::new();
    let runner = MockAgentRunner::new("claude-test");
    let agent = TrogonAgent::new(
        MockSessionNotifier::new(),
        store.clone(),
        runner,
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    )
    .with_compactor(nats.clone());

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            // Subscribe to the compactor subject and reply so compact_messages completes.
            let was_called_inner = was_called.clone();
            let reply_nats = nats.clone();
            let mut sub = nats
                .subscribe("trogon.compactor.compact")
                .await
                .unwrap();
            tokio::task::spawn_local(async move {
                if let Some(msg) = sub.next().await {
                    was_called_inner.store(true, Ordering::SeqCst);
                    let req: serde_json::Value =
                        serde_json::from_slice(&msg.payload).unwrap_or_default();
                    let resp = serde_json::json!({
                        "messages": req["messages"],
                        "compacted": false,
                        "tokens_before": 0,
                        "tokens_after": 0,
                    });
                    if let Some(reply_to) = msg.reply {
                        let _ = reply_nats
                            .publish(reply_to, serde_json::to_vec(&resp).unwrap().into())
                            .await;
                    }
                }
            });

            let resp = agent.new_session(NewSessionRequest::new("/cwd")).await.unwrap();
            let session_id = resp.session_id.to_string();

            // token_budget = 100 → threshold = 85 tokens ≈ 340 bytes of JSON.
            // Pre-populate a message of ~500 chars so estimate already exceeds threshold
            // before the new user message is added.
            let mut state = store.load(&session_id).await.unwrap();
            state.token_budget = 100;
            state.messages = vec![Message::user_text("x".repeat(500))];
            store.save(&session_id, &state).await.unwrap();

            agent
                .prompt(PromptRequest::new(
                    session_id,
                    vec![ContentBlock::Text(TextContent::new("ping"))],
                ))
                .await
                .unwrap();

            // Give the subscriber task a moment to record the call.
            tokio::time::sleep(Duration::from_millis(200)).await;
        })
        .await;

    assert!(
        was_called.load(Ordering::SeqCst),
        "compactor must be called when messages exceed 85% of token_budget"
    );
}

/// Compact is skipped when messages are well below 85 % of token_budget.
#[tokio::test]
async fn compactor_not_called_when_messages_below_85_percent_of_token_budget() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;

    let was_called = Arc::new(AtomicBool::new(false));

    let store = MemorySessionStore::new();
    let runner = MockAgentRunner::new("claude-test");
    let agent = TrogonAgent::new(
        MockSessionNotifier::new(),
        store.clone(),
        runner,
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    )
    .with_compactor(nats.clone());

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            // Subscribe to detect any spurious compact request.
            let was_called_inner = was_called.clone();
            let reply_nats = nats.clone();
            let mut sub = nats
                .subscribe("trogon.compactor.compact")
                .await
                .unwrap();
            tokio::task::spawn_local(async move {
                if let Some(msg) = sub.next().await {
                    was_called_inner.store(true, Ordering::SeqCst);
                    // Reply to avoid hanging compact_messages.
                    let req: serde_json::Value =
                        serde_json::from_slice(&msg.payload).unwrap_or_default();
                    let resp = serde_json::json!({
                        "messages": req["messages"],
                        "compacted": false,
                    });
                    if let Some(reply_to) = msg.reply {
                        let _ = reply_nats
                            .publish(reply_to, serde_json::to_vec(&resp).unwrap().into())
                            .await;
                    }
                }
            });

            let resp = agent.new_session(NewSessionRequest::new("/cwd")).await.unwrap();
            let session_id = resp.session_id.to_string();

            // Default token_budget = 200_000; threshold = 170_000 tokens.
            // A single short message is far below threshold.
            agent
                .prompt(PromptRequest::new(
                    session_id,
                    vec![ContentBlock::Text(TextContent::new("short message"))],
                ))
                .await
                .unwrap();

            // Brief pause to catch any spurious publish.
            tokio::time::sleep(Duration::from_millis(100)).await;
        })
        .await;

    assert!(
        !was_called.load(Ordering::SeqCst),
        "compactor must NOT be called when messages are well below threshold"
    );
}
