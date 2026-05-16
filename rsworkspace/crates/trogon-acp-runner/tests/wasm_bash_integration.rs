//! Integration tests for `WasmRuntimeBashTool` and registry-based bash tool injection.
//!
//! Requires Docker (uses testcontainers to spin up a NATS JetStream server).
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test wasm_bash_integration --features test-helpers

use std::sync::Arc;

use agent_client_protocol::{
    Agent, CreateTerminalResponse, InitializeRequest, NewSessionRequest, PromptRequest,
    ProtocolVersion, TerminalExitStatus, TerminalId, TerminalOutputResponse,
    WaitForTerminalExitResponse,
};
use async_nats::jetstream;
use futures_util::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tokio::sync::RwLock;
use trogon_acp_runner::{
    GatewayConfig, NatsSessionStore, TrogonAgent,
    agent_runner::mock::MockAgentRunner,
    session_notifier::mock::MockSessionNotifier,
    wasm_bash_tool::WasmRuntimeBashTool,
};
use trogon_mcp::McpCallTool;

// ── helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let c = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = c.get_host_port_ipv4(4222).await.unwrap();
    (c, port)
}

async fn connect(port: u16) -> (async_nats::Client, jetstream::Context) {
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js = jetstream::new(nats.clone());
    (nats, js)
}

// ── WasmRuntimeBashTool::call_tool round-trip ────────────────────────────────

/// Spawns mock responders for the four terminal NATS subjects and verifies
/// that `WasmRuntimeBashTool::call_tool` sends them in the correct order and
/// returns the output from the terminal output response.
#[tokio::test]
async fn wasm_bash_tool_call_tool_completes_round_trip() {
    let (_c, port) = start_nats().await;
    let (nats, _js) = connect(port).await;

    let prefix = "acp.wasm";
    let session_id = "test-session-1";
    let terminal_id = "tid-abc";
    let base = format!("{prefix}.session.{session_id}.client.terminal");

    // Subscribe to all subjects upfront so messages cannot arrive before the
    // subscriber is ready (avoids race between tool request and subscriber setup).
    let nats_srv = nats.clone();
    let mut create_sub = nats_srv.subscribe(format!("{base}.create")).await.unwrap();
    let mut wait_sub = nats_srv.subscribe(format!("{base}.wait_for_exit")).await.unwrap();
    let mut output_sub = nats_srv.subscribe(format!("{base}.output")).await.unwrap();
    let mut release_sub = nats_srv.subscribe(format!("{base}.release")).await.unwrap();

    tokio::spawn(async move {
        // 1. create
        if let Some(msg) = create_sub.next().await {
            let resp = CreateTerminalResponse::new(TerminalId::new(terminal_id));
            let payload = serde_json::to_vec(&resp).unwrap();
            nats_srv.publish(msg.reply.unwrap(), payload.into()).await.unwrap();
        }

        // 2. wait_for_exit
        if let Some(msg) = wait_sub.next().await {
            let resp = WaitForTerminalExitResponse::new(
                TerminalExitStatus::new().exit_code(Some(0)),
            );
            let payload = serde_json::to_vec(&resp).unwrap();
            nats_srv.publish(msg.reply.unwrap(), payload.into()).await.unwrap();
        }

        // 3. output
        if let Some(msg) = output_sub.next().await {
            let resp = TerminalOutputResponse::new("hello\n".to_string(), false);
            let payload = serde_json::to_vec(&resp).unwrap();
            nats_srv.publish(msg.reply.unwrap(), payload.into()).await.unwrap();
        }

        // 4. release (best-effort, no reply needed)
        release_sub.next().await;
    });

    let tool = WasmRuntimeBashTool::new(
        nats.clone(),
        prefix,
        session_id,
        std::time::Duration::from_secs(5),
    );

    let args = serde_json::json!({ "command": "echo hello" });
    let result = tool
        .call_tool("bash", &args)
        .await
        .expect("call_tool must succeed");

    assert_eq!(result, "hello\n", "output must match mock terminal response");
}

// ── Registry-based bash tool injection ───────────────────────────────────────

/// Registers a wasm-runtime capability in the registry, creates a `TrogonAgent`
/// with `with_execution_backend`, runs a prompt, and verifies that
/// `add_mcp_tools` was called with a "bash" tool def.
#[tokio::test]
async fn with_execution_backend_injects_bash_tool_when_registry_has_execution_entry() {
    let (_c, port) = start_nats().await;
    let (nats, js) = connect(port).await;

    // Provision and populate the registry.
    let reg_store = trogon_registry::provision(&js)
        .await
        .expect("provision registry");
    let registry = trogon_registry::Registry::new(reg_store);
    let cap = trogon_registry::AgentCapability {
        agent_type: "wasm".to_string(),
        capabilities: vec!["execution".to_string()],
        nats_subject: "acp.wasm.agent.>".to_string(),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": "acp.wasm" }),
    };
    registry.register(&cap).await.expect("register capability");

    // Build the session store and runner.
    let store = NatsSessionStore::open(&js).await.unwrap();
    let runner = MockAgentRunner::new("claude-test");

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let agent = TrogonAgent::new(
                MockSessionNotifier::new(),
                store,
                runner.clone(),
                "acp",
                "claude-test",
                None,
                None,
                Arc::new(RwLock::new(None::<GatewayConfig>)),
            )
            .with_execution_backend(nats.clone(), registry);

            agent
                .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
                .await
                .unwrap();
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();
            agent
                .prompt(PromptRequest::new(session_id, vec![]))
                .await
                .unwrap();
        })
        .await;

    assert!(
        runner.captured_tool_names().contains(&"bash".to_string()),
        "bash tool must be injected when registry has an execution entry, got: {:?}",
        runner.captured_tool_names()
    );
}

// ── WasmRuntimeBashTool error paths ──────────────────────────────────────────

/// `call_tool` with a missing `command` field returns `Err("missing command argument")`
/// without making any NATS calls.
#[tokio::test]
async fn wasm_bash_tool_missing_command_returns_error() {
    let (_c, port) = start_nats().await;
    let (nats, _js) = connect(port).await;

    let tool = WasmRuntimeBashTool::new(
        nats,
        "acp.wasm",
        "test-session-miss",
        std::time::Duration::from_secs(5),
    );

    let result = tool.call_tool("bash", &serde_json::json!({})).await;
    assert!(result.is_err(), "expected Err for missing command");
    assert_eq!(result.unwrap_err(), "missing command argument");
}

/// `call_tool` with a valid command but no NATS responder returns an error.
#[tokio::test]
async fn wasm_bash_tool_no_responder_returns_error() {
    let (_c, port) = start_nats().await;
    let (nats, _js) = connect(port).await;

    let tool = WasmRuntimeBashTool::new(
        nats,
        "acp.wasm",
        "test-session-noresp",
        std::time::Duration::from_secs(5),
    );

    let result = tool
        .call_tool("bash", &serde_json::json!({ "command": "echo hi" }))
        .await;
    assert!(result.is_err(), "expected Err when no responder is set up");
    assert!(
        !result.unwrap_err().is_empty(),
        "error message must not be empty"
    );
}

/// When the registry has no "execution" entries, no bash tool is injected and
/// the prompt completes normally.
#[tokio::test]
async fn with_execution_backend_no_bash_tool_when_registry_empty() {
    let (_c, port) = start_nats().await;
    let (nats, js) = connect(port).await;

    let reg_store = trogon_registry::provision(&js)
        .await
        .expect("provision registry");
    let registry = trogon_registry::Registry::new(reg_store);
    // Do NOT register any capability — registry is empty.

    let store = NatsSessionStore::open(&js).await.unwrap();
    let runner = MockAgentRunner::new("claude-test");

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let agent = TrogonAgent::new(
                MockSessionNotifier::new(),
                store,
                runner.clone(),
                "acp",
                "claude-test",
                None,
                None,
                Arc::new(RwLock::new(None::<GatewayConfig>)),
            )
            .with_execution_backend(nats.clone(), registry);

            let resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();
            let result = agent
                .prompt(PromptRequest::new(session_id, vec![]))
                .await;
            assert!(result.is_ok(), "prompt must succeed even without execution backend");
        })
        .await;

    assert!(
        !runner.captured_tool_names().contains(&"bash".to_string()),
        "bash tool must NOT be injected when registry is empty, got: {:?}",
        runner.captured_tool_names()
    );
}

// ── acp_prefix fallback ───────────────────────────────────────────────────────

/// When the registry entry has no `acp_prefix` key in metadata, the code falls
/// back to `"acp.wasm"` and still injects the bash tool.
#[tokio::test]
async fn with_execution_backend_injects_bash_tool_when_acp_prefix_absent_from_metadata() {
    let (_c, port) = start_nats().await;
    let (nats, js) = connect(port).await;

    let reg_store = trogon_registry::provision(&js)
        .await
        .expect("provision registry");
    let registry = trogon_registry::Registry::new(reg_store);

    let cap = trogon_registry::AgentCapability {
        agent_type: "wasm".to_string(),
        capabilities: vec!["execution".to_string()],
        nats_subject: "acp.wasm.agent.>".to_string(),
        current_load: 0,
        metadata: serde_json::json!({}), // no acp_prefix key
    };
    registry.register(&cap).await.expect("register capability");

    let store = NatsSessionStore::open(&js).await.unwrap();
    let runner = MockAgentRunner::new("claude-test");

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let agent = TrogonAgent::new(
                MockSessionNotifier::new(),
                store,
                runner.clone(),
                "acp",
                "claude-test",
                None,
                None,
                Arc::new(RwLock::new(None::<GatewayConfig>)),
            )
            .with_execution_backend(nats.clone(), registry);

            agent
                .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
                .await
                .unwrap();
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();
            agent
                .prompt(PromptRequest::new(session_id, vec![]))
                .await
                .unwrap();
        })
        .await;

    assert!(
        runner.captured_tool_names().contains(&"bash".to_string()),
        "bash tool must be injected using the default prefix when acp_prefix is absent, got: {:?}",
        runner.captured_tool_names()
    );
}

// ── multiple execution entries ────────────────────────────────────────────────

/// When multiple execution entries are in the registry, only the first is used
/// — the bash tool is injected exactly once, not once per entry.
#[tokio::test]
async fn with_execution_backend_injects_bash_tool_exactly_once_with_multiple_entries() {
    let (_c, port) = start_nats().await;
    let (nats, js) = connect(port).await;

    let reg_store = trogon_registry::provision(&js)
        .await
        .expect("provision registry");
    let registry = trogon_registry::Registry::new(reg_store);

    for i in 0..2u8 {
        let cap = trogon_registry::AgentCapability {
            agent_type: format!("wasm-{i}"),
            capabilities: vec!["execution".to_string()],
            nats_subject: format!("acp.wasm{i}.agent.>"),
            current_load: 0,
            metadata: serde_json::json!({ "acp_prefix": format!("acp.wasm{i}") }),
        };
        registry.register(&cap).await.expect("register capability");
    }

    let store = NatsSessionStore::open(&js).await.unwrap();
    let runner = MockAgentRunner::new("claude-test");

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let agent = TrogonAgent::new(
                MockSessionNotifier::new(),
                store,
                runner.clone(),
                "acp",
                "claude-test",
                None,
                None,
                Arc::new(RwLock::new(None::<GatewayConfig>)),
            )
            .with_execution_backend(nats.clone(), registry);

            agent
                .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
                .await
                .unwrap();
            let resp = agent
                .new_session(NewSessionRequest::new("/cwd"))
                .await
                .unwrap();
            let session_id = resp.session_id.to_string();
            agent
                .prompt(PromptRequest::new(session_id, vec![]))
                .await
                .unwrap();
        })
        .await;

    let bash_count = runner
        .captured_tool_names()
        .iter()
        .filter(|n| *n == "bash")
        .count();
    assert_eq!(
        bash_count, 1,
        "bash tool must be injected exactly once even with multiple execution entries, got: {:?}",
        runner.captured_tool_names()
    );
}

// ── tool_def shape ────────────────────────────────────────────────────────────

/// `WasmRuntimeBashTool::tool_def()` returns a well-formed `ToolDef`: name is
/// `"bash"`, the input schema lists `command` as a required string property.
#[test]
fn wasm_bash_tool_def_has_correct_shape() {
    let def = WasmRuntimeBashTool::tool_def();

    assert_eq!(def.name, "bash");

    let required = &def.input_schema["required"];
    assert!(
        required
            .as_array()
            .map_or(false, |arr| arr.iter().any(|v| v.as_str() == Some("command"))),
        "input_schema must list 'command' as required, got: {}",
        def.input_schema
    );

    let cmd_type = &def.input_schema["properties"]["command"]["type"];
    assert_eq!(
        cmd_type.as_str(),
        Some("string"),
        "command property must be type string, got: {}",
        cmd_type
    );
}
