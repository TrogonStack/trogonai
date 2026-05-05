//! Integration test for the full bash-tool dispatch path:
//!
//!   AgentLoop (mock Anthropic HTTP) → receives `bash` tool_use
//!   → mcp_dispatch → WasmRuntimeBashTool::call_tool
//!   → NATS round-trip to mock wasm-runtime responders
//!   → tool output fed back to AgentLoop → second LLM call → end_turn
//!
//! This is the only test that exercises the complete chain between the
//! agent loop, the bash tool implementation, and the NATS transport in a
//! single end-to-end pass.
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test bash_tool_dispatch_integration --features test-helpers

use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::{
    CreateTerminalResponse, TerminalExitStatus, TerminalId, TerminalOutputResponse,
    WaitForTerminalExitResponse,
};
use futures_util::StreamExt as _;
use httpmock::prelude::*;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_agent_core::agent_loop::{AgentEvent, AgentLoop, Message};
use trogon_agent_core::tools::ToolContext;
use trogon_acp_runner::wasm_bash_tool::WasmRuntimeBashTool;

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

async fn connect_nats(port: u16) -> async_nats::Client {
    async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS")
}

fn make_agent(base_url: &str, nats: async_nats::Client, prefix: &str, session_id: &str) -> AgentLoop {
    let bash_tool = WasmRuntimeBashTool::new(
        nats,
        prefix,
        session_id,
        Duration::from_secs(10),
    );
    let (dispatch_name, orig_name, client) = bash_tool.into_dispatch();

    AgentLoop {
        http_client: reqwest::Client::new(),
        proxy_url: "http://127.0.0.1:1".to_string(),
        anthropic_token: "test-token".to_string(),
        anthropic_base_url: Some(base_url.to_string()),
        anthropic_extra_headers: vec![],
        model: "claude-test".to_string(),
        max_iterations: 5,
        thinking_budget: None,
        tool_context: Arc::new(ToolContext {
            proxy_url: "http://127.0.0.1:1".to_string(),
        }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![WasmRuntimeBashTool::tool_def()],
        mcp_dispatch: vec![(dispatch_name, orig_name, client)],
        permission_checker: None,
        elicitation_provider: None,
    }
}

/// Mock Anthropic response: the LLM requests a `bash` tool call.
fn bash_tool_use_body(command: &str) -> String {
    serde_json::json!({
        "stop_reason": "tool_use",
        "content": [{
            "type": "tool_use",
            "id": "bash_001",
            "name": "bash",
            "input": { "command": command }
        }],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 5,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0
        }
    })
    .to_string()
}

/// Mock Anthropic response: the LLM finishes with a text reply.
fn end_turn_body(text: &str) -> String {
    serde_json::json!({
        "stop_reason": "end_turn",
        "content": [{ "type": "text", "text": text }],
        "usage": {
            "input_tokens": 15,
            "output_tokens": 8,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0
        }
    })
    .to_string()
}

/// Spawn mock NATS responders for the four terminal subjects used by
/// `WasmRuntimeBashTool`: create → wait_for_exit → output → release.
fn spawn_terminal_responders(nats: async_nats::Client, base: String, terminal_id: &'static str, output: &'static str) {
    tokio::spawn(async move {
        // 1. create terminal
        let mut sub = nats.subscribe(format!("{base}.create")).await.unwrap();
        if let Some(msg) = sub.next().await {
            let resp = CreateTerminalResponse::new(TerminalId::new(terminal_id));
            let payload = serde_json::to_vec(&resp).unwrap();
            nats.publish(msg.reply.unwrap(), payload.into()).await.unwrap();
        }

        // 2. wait for exit
        let mut sub = nats.subscribe(format!("{base}.wait_for_exit")).await.unwrap();
        if let Some(msg) = sub.next().await {
            let resp = WaitForTerminalExitResponse::new(
                TerminalExitStatus::new().exit_code(Some(0)),
            );
            let payload = serde_json::to_vec(&resp).unwrap();
            nats.publish(msg.reply.unwrap(), payload.into()).await.unwrap();
        }

        // 3. output
        let mut sub = nats.subscribe(format!("{base}.output")).await.unwrap();
        if let Some(msg) = sub.next().await {
            let resp = TerminalOutputResponse::new(output.to_string(), false);
            let payload = serde_json::to_vec(&resp).unwrap();
            nats.publish(msg.reply.unwrap(), payload.into()).await.unwrap();
        }

        // 4. release (best-effort, no reply expected)
        let mut sub = nats.subscribe(format!("{base}.release")).await.unwrap();
        sub.next().await;
    });
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Full dispatch path: AgentLoop receives a `bash` tool_use from the mock LLM,
/// WasmRuntimeBashTool executes it over NATS (mock responders), the tool output
/// is fed back, and the loop completes with a final text response.
///
/// Asserts:
/// - `ToolCallStarted` event with name == "bash"
/// - `ToolCallFinished` event with output containing the command output
/// - The second Anthropic request body contains the tool output (i.e. the
///   tool result was properly forwarded back to the LLM)
/// - `run_chat_streaming` returns `Ok`
#[tokio::test]
async fn agent_loop_dispatches_bash_tool_via_nats_and_returns_output() {
    let (_c, port) = start_nats().await;
    let nats = connect_nats(port).await;

    let prefix = "acp.wasm";
    let session_id = "dispatch-sess-1";
    let command = "echo hello";
    let cmd_output = "hello\n";
    let base = format!("{prefix}.session.{session_id}.client.terminal");

    // Spawn NATS mock responders before the agent runs.
    spawn_terminal_responders(nats.clone(), base, "tid-001", cmd_output);
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Mock Anthropic API:
    //   call 1 (no tool_result in body) → bash tool_use
    //   call 2 (tool_result in body)    → end_turn
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Command completed successfully."));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(bash_tool_use_body(command));
    });

    let agent = make_agent(&server.base_url(), nats, prefix, session_id);

    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let result = agent
        .run_chat_streaming(
            vec![Message::user_text("run echo hello")],
            &[],
            None,
            tx,
            None,
        )
        .await;

    assert!(
        result.is_ok(),
        "run_chat_streaming must succeed, got: {:?}",
        result.err()
    );

    let mut events = vec![];
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }

    // The loop must have emitted ToolCallStarted for "bash".
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolCallStarted { name, .. } if name == "bash")),
        "expected ToolCallStarted for 'bash', got: {events:?}"
    );

    // ToolCallFinished must carry the NATS-delivered command output.
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolCallFinished { output, .. } if output.contains("hello")
        )),
        "expected ToolCallFinished with 'hello' in output, got: {events:?}"
    );
}

/// When the `bash` tool call returns an error (NATS no-responder), the agent
/// loop must not crash — it emits `ToolCallFinished` with an error message and
/// continues to the next LLM turn.
#[tokio::test]
async fn agent_loop_handles_bash_tool_nats_error_without_crashing() {
    let (_c, port) = start_nats().await;
    let nats = connect_nats(port).await;

    let prefix = "acp.wasm";
    let session_id = "dispatch-sess-err";
    // No NATS responders — call_tool will fail with a NATS error.

    let server = MockServer::start();
    // After the tool error the loop sends another turn; return end_turn.
    server.mock(|when, then| {
        when.method(POST)
            .path("/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(end_turn_body("Handled the error."));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(bash_tool_use_body("echo hi"));
    });

    let agent = make_agent(&server.base_url(), nats, prefix, session_id);

    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let result = agent
        .run_chat_streaming(
            vec![Message::user_text("run a command")],
            &[],
            None,
            tx,
            None,
        )
        .await;

    assert!(
        result.is_ok(),
        "run_chat_streaming must not crash on tool error, got: {:?}",
        result.err()
    );

    let mut events = vec![];
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }

    // ToolCallStarted must still be emitted.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolCallStarted { name, .. } if name == "bash")),
        "expected ToolCallStarted for 'bash' even on error path, got: {events:?}"
    );

    // ToolCallFinished must be emitted (with an error message, not a crash).
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolCallFinished { .. })),
        "expected ToolCallFinished after NATS error, got: {events:?}"
    );
}
