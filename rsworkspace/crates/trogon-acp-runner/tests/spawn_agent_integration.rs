//! Integration tests for `SpawnAgentTool::call_tool` — requires Docker.
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test spawn_agent_integration --features test-helpers

use futures_util::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_acp_runner::spawn_agent_tool::SpawnAgentTool;
use trogon_mcp::McpCallTool;

async fn start_nats() -> (ContainerAsync<Nats>, async_nats::Client) {
    let c = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = c.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    (c, nats)
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Spawn a subscriber that replies to the first message on `subject` with `reply_body`.
fn spawn_responder(nats: async_nats::Client, subject: String, reply_body: &'static str) {
    tokio::spawn(async move {
        let mut sub = nats.subscribe(subject).await.unwrap();
        if let Some(msg) = sub.next().await {
            if let Some(reply) = msg.reply {
                nats.publish(reply, reply_body.into()).await.unwrap();
            }
        }
    });
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn spawn_agent_tool_returns_responder_output() {
    let (_c, nats) = start_nats().await;
    let prefix = "trogon.test";

    spawn_responder(
        nats.clone(),
        format!("{prefix}.agent.spawn"),
        "Explore agent output: found 3 files",
    );

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let tool = SpawnAgentTool::new(nats, prefix);
    let result = tool
        .call_tool(
            "spawn_agent",
            &serde_json::json!({"capability": "explore", "prompt": "find all rust files"}),
        )
        .await
        .unwrap();

    assert!(
        result.contains("Explore agent output"),
        "expected responder body, got: {result}"
    );
}

#[tokio::test]
async fn spawn_agent_tool_forwards_capability_and_prompt_in_payload() {
    let (_c, nats) = start_nats().await;
    let prefix = "trogon.test2";

    let nats_clone = nats.clone();
    tokio::spawn(async move {
        let mut sub = nats_clone
            .subscribe(format!("{prefix}.agent.spawn"))
            .await
            .unwrap();
        if let Some(msg) = sub.next().await {
            // Parse the payload and echo it back so the test can inspect it.
            let body: serde_json::Value =
                serde_json::from_slice(&msg.payload).unwrap_or_default();
            let echo = serde_json::to_string(&body).unwrap();
            if let Some(reply) = msg.reply {
                nats_clone.publish(reply, echo.into()).await.unwrap();
            }
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let tool = SpawnAgentTool::new(nats, prefix);
    let result = tool
        .call_tool(
            "spawn_agent",
            &serde_json::json!({"capability": "plan", "prompt": "design the auth module"}),
        )
        .await
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["capability"].as_str(), Some("plan"));
    assert_eq!(parsed["prompt"].as_str(), Some("design the auth module"));
}

#[tokio::test]
async fn spawn_agent_tool_times_out_when_no_responder() {
    let (_c, nats) = start_nats().await;
    let prefix = "trogon.noresponder";

    // SpawnAgentTool has a 120 s timeout — too long for a test.
    // We verify it returns Err when NATS has no subscriber (no-responder error).
    let tool = SpawnAgentTool::new(nats, prefix);
    let result = tool
        .call_tool(
            "spawn_agent",
            &serde_json::json!({"capability": "explore", "prompt": "anything"}),
        )
        .await;

    assert!(result.is_err(), "must fail when no responder is present");
}
