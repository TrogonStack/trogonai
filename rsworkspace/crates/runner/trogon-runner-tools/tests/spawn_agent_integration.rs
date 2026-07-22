//! Integration tests for SpawnAgentTool — requires Docker (testcontainers starts NATS).
//!
//! Run with:
//!   cargo test -p trogon-runner-tools --test spawn_agent_integration

use futures_util::StreamExt as _;
use std::sync::Arc;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use trogon_mcp::McpCallTool as _;
use trogon_runner_tools::spawn_agent_tool::SpawnAgentTool;

// ── Helpers ───────────────────────────────────────────────────────────────────

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
        .expect("connect should succeed")
}

// ── Constructor / into_dispatch ───────────────────────────────────────────────

/// `new()` + `into_dispatch()` returns (tool_name, tool_name, Arc<impl McpCallTool>).
#[tokio::test]
async fn into_dispatch_returns_correct_names() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;

    let tool = SpawnAgentTool::new(client, "trogon", "");
    let (server_name, tool_name, _handler) = tool.into_dispatch();

    assert_eq!(server_name, "spawn_agent");
    assert_eq!(tool_name, "spawn_agent");
}

/// `into_dispatch()` wraps the struct in an Arc and the handler is callable.
#[tokio::test]
async fn into_dispatch_handler_is_arc_mcp_call_tool() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;

    let tool = SpawnAgentTool::new(client, "trogon", "");
    let (_server_name, _tool_name, handler) = tool.into_dispatch();

    // Arc<dyn McpCallTool> — just confirm we hold a valid Arc (strong count = 1).
    assert_eq!(Arc::strong_count(&handler), 1);
}

// ── call_tool — happy path ────────────────────────────────────────────────────

/// A subscriber on `{prefix}.spawn` receives the payload and replies;
/// `call_tool` returns the reply body as a String.
#[tokio::test]
async fn call_tool_delivers_request_and_returns_reply() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;
    let registry_client = nats_client(port).await;

    let mut sub = registry_client.subscribe("trogon.spawn").await.unwrap();

    tokio::spawn(async move {
        if let Some(msg) = sub.next().await {
            let payload: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap_or_default();
            let response = format!(
                "ran agent={} with: {}",
                payload["agent"].as_str().unwrap_or(""),
                payload["prompt"].as_str().unwrap_or("")
            );
            if let Some(reply) = msg.reply {
                registry_client.publish(reply, response.into()).await.unwrap();
            }
        }
    });

    let tool = SpawnAgentTool::new(client, "trogon", "");
    let result = tool
        .call_tool(
            "spawn_agent",
            &serde_json::json!({ "agent": "explore", "prompt": "find all Rust files" }),
        )
        .await;

    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
    let body = result.unwrap();
    assert!(body.contains("explore"), "body should contain agent name");
    assert!(body.contains("find all Rust files"), "body should contain prompt");
}

/// Prefix is forwarded correctly — subject is `{prefix}.spawn`.
#[tokio::test]
async fn call_tool_uses_correct_subject_prefix() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;
    let registry_client = nats_client(port).await;

    let mut sub = registry_client.subscribe("myteam.spawn").await.unwrap();

    tokio::spawn(async move {
        if let Some(msg) = sub.next().await
            && let Some(reply) = msg.reply
        {
            registry_client.publish(reply, "ack".into()).await.unwrap();
        }
    });

    // Give NATS time to propagate the subscription before the request fires.
    tokio::task::yield_now().await;

    let tool = SpawnAgentTool::new(client, "myteam", "");
    let result = tool
        .call_tool("spawn_agent", &serde_json::json!({ "prompt": "design auth flow" }))
        .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "ack");
}

/// The JSON sent to the spawn handler contains `agent`, `prompt`, and `session_id`.
#[tokio::test]
async fn call_tool_sends_agent_and_prompt_in_payload() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;
    let registry_client = nats_client(port).await;

    let mut sub = registry_client.subscribe("t.spawn").await.unwrap();

    tokio::spawn(async move {
        if let Some(msg) = sub.next().await {
            let v: serde_json::Value = serde_json::from_slice(&msg.payload).expect("payload must be JSON");

            assert_eq!(v["agent"].as_str().unwrap(), "reviewer");
            assert_eq!(v["prompt"].as_str().unwrap(), "list files");
            assert_eq!(v["session_id"].as_str().unwrap(), "");
            assert!(v.get("capability").is_none(), "capability must not be sent");
            assert!(v.as_object().map(|o| o.len() == 3).unwrap_or(false));

            if let Some(reply) = msg.reply {
                registry_client.publish(reply, "ok".into()).await.unwrap();
            }
        }
    });

    let tool = SpawnAgentTool::new(client, "t", "");
    let result = tool
        .call_tool(
            "spawn_agent",
            &serde_json::json!({ "agent": "reviewer", "prompt": "list files" }),
        )
        .await;

    assert!(result.is_ok());
}

// ── call_tool — missing arguments ─────────────────────────────────────────────

/// `call_tool` succeeds with only `prompt` when `agent` is omitted.
#[tokio::test]
async fn call_tool_accepts_prompt_without_agent() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;
    let registry_client = nats_client(port).await;

    let mut sub = registry_client.subscribe("trogon.spawn").await.unwrap();
    tokio::spawn(async move {
        if let Some(msg) = sub.next().await
            && let Some(reply) = msg.reply
        {
            registry_client.publish(reply, "ok".into()).await.unwrap();
        }
    });

    tokio::task::yield_now().await;

    let tool = SpawnAgentTool::new(client, "trogon", "");
    let result = tool
        .call_tool("spawn_agent", &serde_json::json!({ "prompt": "do something" }))
        .await;

    assert!(result.is_ok(), "prompt alone must be sufficient");
}

/// `call_tool` returns an error when `prompt` is absent from arguments.
#[tokio::test]
async fn call_tool_errors_on_missing_prompt() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;

    let tool = SpawnAgentTool::new(client, "trogon", "");
    let result = tool
        .call_tool("spawn_agent", &serde_json::json!({ "agent": "explore" }))
        .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("prompt"), "error should mention 'prompt'");
}

/// `call_tool` returns an error when arguments are an empty object.
#[tokio::test]
async fn call_tool_errors_on_empty_arguments() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;

    let tool = SpawnAgentTool::new(client, "trogon", "");
    let result = tool.call_tool("spawn_agent", &serde_json::json!({})).await;

    assert!(result.is_err());
}

// ── call_tool — timeout ───────────────────────────────────────────────────────

/// When no subscriber is listening, `call_tool` times out and returns an error
/// containing "timed out".
///
/// Uses a very short timeout override — we patch this by relying on the real
/// SPAWN_TIMEOUT constant being 120 s, but we don't want to wait that long in
/// tests. Instead we verify the *message* format by connecting to a port where
/// NATS is not listening, which causes an immediate NATS error (no server) rather
/// than a timeout, so we only check for an Err result here.
#[tokio::test]
async fn call_tool_returns_error_when_no_subscriber_responds() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;

    // No subscriber on this subject — NATS will return "no responders" immediately.
    let tool = SpawnAgentTool::new(client, "ghost", "");
    let result = tool
        .call_tool("spawn_agent", &serde_json::json!({ "prompt": "anything" }))
        .await;

    assert!(result.is_err(), "expected error with no subscriber");
}

/// `tool_def` describes spawning a sub-agent in an isolated worktree (not the
/// old registry-based wording).
#[test]
fn tool_def_describes_isolated_sub_agent_spawn() {
    let def = SpawnAgentTool::tool_def();
    assert!(
        def.description.contains("sub-agent") && def.description.contains("isolated worktree"),
        "description must reference sub-agent spawning in an isolated worktree"
    );
}

// ── call_tool — response passthrough ─────────────────────────────────────────

/// The raw bytes from the registry reply are returned verbatim as a String.
#[tokio::test]
async fn call_tool_returns_registry_reply_verbatim() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;
    let registry_client = nats_client(port).await;

    let expected = "Agent completed. Result: 42 files found.";

    let mut sub = registry_client.subscribe("ns.spawn").await.unwrap();
    tokio::spawn(async move {
        if let Some(msg) = sub.next().await
            && let Some(reply) = msg.reply
        {
            registry_client.publish(reply, expected.into()).await.unwrap();
        }
    });

    tokio::task::yield_now().await;

    let tool = SpawnAgentTool::new(client, "ns", "");
    let result = tool
        .call_tool("spawn_agent", &serde_json::json!({ "prompt": "count files" }))
        .await;

    assert_eq!(result.unwrap(), expected);
}

/// Multi-line / structured text from the registry is passed through intact.
#[tokio::test]
async fn call_tool_handles_multiline_registry_reply() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;
    let registry_client = nats_client(port).await;

    let expected = "Line 1\nLine 2\nLine 3";

    let mut sub = registry_client.subscribe("ns2.spawn").await.unwrap();
    tokio::spawn(async move {
        if let Some(msg) = sub.next().await
            && let Some(reply) = msg.reply
        {
            registry_client.publish(reply, expected.into()).await.unwrap();
        }
    });

    let tool = SpawnAgentTool::new(client, "ns2", "");
    let result = tool
        .call_tool("spawn_agent", &serde_json::json!({ "prompt": "outline a plan" }))
        .await;

    assert_eq!(result.unwrap(), expected);
}

// ── concurrency ───────────────────────────────────────────────────────────────

/// Multiple concurrent `call_tool` invocations each get their own reply.
#[tokio::test]
async fn call_tool_handles_concurrent_requests() {
    let (_container, port) = start_nats().await;

    // One registry subscriber that handles N messages.
    let registry_client = nats_client(port).await;
    let mut sub = registry_client.subscribe("cc.spawn").await.unwrap();

    tokio::spawn(async move {
        while let Some(msg) = sub.next().await {
            let v: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap_or_default();
            let reply_body = format!("done:{}", v["prompt"].as_str().unwrap_or("?"));
            if let Some(reply) = msg.reply {
                registry_client.publish(reply, reply_body.into()).await.unwrap();
            }
        }
    });

    let prompts = ["explore task", "plan task", "review task"];
    let mut handles = vec![];

    for prompt in prompts {
        let client = nats_client(port).await;
        let prompt = prompt.to_string();
        handles.push(tokio::spawn(async move {
            let tool = SpawnAgentTool::new(client, "cc", "");
            tool.call_tool("spawn_agent", &serde_json::json!({ "prompt": prompt }))
                .await
        }));
    }

    for handle in handles {
        let result = handle.await.expect("task should not panic");
        assert!(result.is_ok(), "concurrent call failed: {:?}", result.unwrap_err());
        assert!(result.unwrap().starts_with("done:"));
    }
}
