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

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::{CreateTerminalResponse, TerminalId};
use futures_util::StreamExt as _;
use httpmock::prelude::*;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_agent_core::agent_loop::{AgentEvent, AgentLoop, Message};
use trogon_agent_core::tools::ToolContext;
use trogon_acp_runner::session_store::mock::MemorySessionStore;
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
        std::path::PathBuf::from("/tmp"),
        Duration::from_secs(10),
        MemorySessionStore::new(),
    );
    let (dispatch_name, orig_name, client) = bash_tool.into_dispatch();

    AgentLoop {
        http_client: reqwest::Client::new(),
        proxy_url: "http://127.0.0.1:1".to_string(),
        anthropic_token: "test-token".to_string(),
        streaming_client: None,
        anthropic_base_url: Some(base_url.to_string()),
        anthropic_extra_headers: vec![],
        model: "claude-test".to_string(),
        max_iterations: 5,
        thinking_budget: None,
        tool_context: Arc::new(ToolContext {
            proxy_url: "http://127.0.0.1:1".to_string(),
            cwd: ".".to_string(),
            http_client: reqwest::Client::new(),
        }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![WasmRuntimeBashTool::<MemorySessionStore>::tool_def()],
        mcp_dispatch: vec![(dispatch_name, orig_name, client)],
        permission_checker: None,
        elicitation_provider: None,
    }
}

fn sse_event(event_type: &str, data: serde_json::Value) -> String {
    format!(
        "event: {event_type}\ndata: {}\n\n",
        serde_json::to_string(&data).unwrap()
    )
}

/// SSE stream: the LLM requests a `bash` tool call with the given command.
fn sse_bash_tool_use(command: &str) -> String {
    [
        sse_event("message_start", serde_json::json!({
            "type": "message_start",
            "message": {"usage": {"input_tokens": 10, "output_tokens": 0,
                                  "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}
        })),
        sse_event("content_block_start", serde_json::json!({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "tool_use", "id": "bash_001", "name": "bash"}
        })),
        sse_event("content_block_delta", serde_json::json!({
            "type": "content_block_delta", "index": 0,
            "delta": {
                "type": "input_json_delta",
                "partial_json": serde_json::json!({"command": command}).to_string()
            }
        })),
        sse_event("content_block_stop", serde_json::json!({"type": "content_block_stop", "index": 0})),
        sse_event("message_delta", serde_json::json!({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use"},
            "usage": {"output_tokens": 5}
        })),
        sse_event("message_stop", serde_json::json!({"type": "message_stop"})),
    ]
    .join("")
}

/// SSE stream: the LLM finishes with a text reply.
fn sse_end_turn(text: &str) -> String {
    [
        sse_event("message_start", serde_json::json!({
            "type": "message_start",
            "message": {"usage": {"input_tokens": 15, "output_tokens": 0,
                                  "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}
        })),
        sse_event("content_block_start", serde_json::json!({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "text", "text": ""}
        })),
        sse_event("content_block_delta", serde_json::json!({
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": text}
        })),
        sse_event("content_block_stop", serde_json::json!({"type": "content_block_stop", "index": 0})),
        sse_event("message_delta", serde_json::json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": 8}
        })),
        sse_event("message_stop", serde_json::json!({"type": "message_stop"})),
    ]
    .join("")
}

/// Spawn mock NATS responders for the four interactions used by the
/// demarcation-marker protocol: create → output(baseline) → write_stdin → output(poll).
fn spawn_terminal_responders(
    nats: async_nats::Client,
    base: String,
    ext_base: String,
    terminal_id: &'static str,
    output: &'static str,
) {
    tokio::spawn(async move {
        let mut create_sub = nats.subscribe(format!("{base}.create")).await.unwrap();
        let mut output_sub = nats.subscribe(format!("{base}.output")).await.unwrap();
        let mut write_sub = nats
            .subscribe(format!("{ext_base}.terminal.write_stdin"))
            .await
            .unwrap();

        // 1. terminal.create
        if let Some(msg) = create_sub.next().await {
            let resp = CreateTerminalResponse::new(TerminalId::new(terminal_id));
            let payload = serde_json::to_vec(&resp).unwrap();
            nats.publish(msg.reply.unwrap(), payload.into()).await.unwrap();
        }

        // 2. terminal.output — baseline (empty)
        if let Some(msg) = output_sub.next().await {
            let payload = serde_json::to_vec(&serde_json::json!({"output": ""})).unwrap();
            nats.publish(msg.reply.unwrap(), payload.into()).await.unwrap();
        }

        // 3. ext.terminal.write_stdin — ACK
        if let Some(msg) = write_sub.next().await {
            let payload = serde_json::to_vec(&serde_json::json!({"ok": true})).unwrap();
            nats.publish(msg.reply.unwrap(), payload.into()).await.unwrap();
        }

        // 4. terminal.output — poll with demarcation marker
        if let Some(msg) = output_sub.next().await {
            let full = format!("{output}__EXIT_0__\n");
            let payload = serde_json::to_vec(&serde_json::json!({"output": full})).unwrap();
            nats.publish(msg.reply.unwrap(), payload.into()).await.unwrap();
        }
    });
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// SSE bash tool_use with a configurable tool ID (needed when testing two
/// sequential bash calls so the agent can distinguish tool_results by ID).
fn sse_bash_tool_use_with_id(command: &str, tool_id: &str) -> String {
    [
        sse_event("message_start", serde_json::json!({
            "type": "message_start",
            "message": {"usage": {"input_tokens": 10, "output_tokens": 0,
                                  "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}
        })),
        sse_event("content_block_start", serde_json::json!({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "tool_use", "id": tool_id, "name": "bash"}
        })),
        sse_event("content_block_delta", serde_json::json!({
            "type": "content_block_delta", "index": 0,
            "delta": {
                "type": "input_json_delta",
                "partial_json": serde_json::json!({"command": command}).to_string()
            }
        })),
        sse_event("content_block_stop", serde_json::json!({"type": "content_block_stop", "index": 0})),
        sse_event("message_delta", serde_json::json!({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use"},
            "usage": {"output_tokens": 5}
        })),
        sse_event("message_stop", serde_json::json!({"type": "message_stop"})),
    ]
    .join("")
}

/// Spawns NATS responders for two sequential bash calls where the second call
/// reuses the terminal created in the first.
///
/// Protocol for two calls:
///   Call 1: create → output(baseline="") → write_stdin → output(poll with marker)
///   Call 2: (no create) → output(baseline=accumulated) → write_stdin → output(poll+marker)
///
/// Returns an `Arc<AtomicU32>` counting how many times terminal.create was called.
fn spawn_two_bash_responders(
    nats: async_nats::Client,
    base: String,
    ext_base: String,
    terminal_id: &'static str,
) -> Arc<AtomicU32> {
    let create_count = Arc::new(AtomicU32::new(0));
    let cc = create_count.clone();
    tokio::spawn(async move {
        let mut create_sub = nats.subscribe(format!("{base}.create")).await.unwrap();
        let mut output_sub = nats.subscribe(format!("{base}.output")).await.unwrap();
        let mut write_sub = nats
            .subscribe(format!("{ext_base}.terminal.write_stdin"))
            .await
            .unwrap();

        // ── First bash call: create → baseline → write → poll ─────────────
        if let Some(msg) = create_sub.next().await {
            cc.fetch_add(1, Ordering::SeqCst);
            let resp = CreateTerminalResponse::new(TerminalId::new(terminal_id));
            nats.publish(msg.reply.unwrap(), serde_json::to_vec(&resp).unwrap().into())
                .await
                .unwrap();
        }
        if let Some(msg) = output_sub.next().await {
            let p = serde_json::to_vec(&serde_json::json!({"output": ""})).unwrap();
            nats.publish(msg.reply.unwrap(), p.into()).await.unwrap();
        }
        if let Some(msg) = write_sub.next().await {
            let p = serde_json::to_vec(&serde_json::json!({"ok": true})).unwrap();
            nats.publish(msg.reply.unwrap(), p.into()).await.unwrap();
        }
        if let Some(msg) = output_sub.next().await {
            let p = serde_json::to_vec(
                &serde_json::json!({"output": "first-out\n__EXIT_0__\n"}),
            )
            .unwrap();
            nats.publish(msg.reply.unwrap(), p.into()).await.unwrap();
        }

        // ── Second bash call: NO create → baseline → write → poll ─────────
        if let Some(msg) = output_sub.next().await {
            // Baseline = accumulated output from first call.
            let p = serde_json::to_vec(
                &serde_json::json!({"output": "first-out\n__EXIT_0__\n"}),
            )
            .unwrap();
            nats.publish(msg.reply.unwrap(), p.into()).await.unwrap();
        }
        if let Some(msg) = write_sub.next().await {
            let p = serde_json::to_vec(&serde_json::json!({"ok": true})).unwrap();
            nats.publish(msg.reply.unwrap(), p.into()).await.unwrap();
        }
        if let Some(msg) = output_sub.next().await {
            let p = serde_json::to_vec(&serde_json::json!({
                "output": "first-out\n__EXIT_0__\nsecond-out\n__EXIT_0__\n"
            }))
            .unwrap();
            nats.publish(msg.reply.unwrap(), p.into()).await.unwrap();
        }
    });
    create_count
}

/// Spawns NATS responders that capture the raw payload of the `terminal.create`
/// message so the test can inspect its contents (e.g. the `cwd` field).
fn spawn_terminal_responders_capture_create(
    nats: async_nats::Client,
    base: String,
    ext_base: String,
    terminal_id: &'static str,
    output: &'static str,
) -> Arc<Mutex<Option<Vec<u8>>>> {
    let captured: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let cap = captured.clone();
    tokio::spawn(async move {
        let mut create_sub = nats.subscribe(format!("{base}.create")).await.unwrap();
        let mut output_sub = nats.subscribe(format!("{base}.output")).await.unwrap();
        let mut write_sub = nats
            .subscribe(format!("{ext_base}.terminal.write_stdin"))
            .await
            .unwrap();

        if let Some(msg) = create_sub.next().await {
            *cap.lock().unwrap() = Some(msg.payload.to_vec());
            let resp = CreateTerminalResponse::new(TerminalId::new(terminal_id));
            nats.publish(msg.reply.unwrap(), serde_json::to_vec(&resp).unwrap().into())
                .await
                .unwrap();
        }
        if let Some(msg) = output_sub.next().await {
            let p = serde_json::to_vec(&serde_json::json!({"output": ""})).unwrap();
            nats.publish(msg.reply.unwrap(), p.into()).await.unwrap();
        }
        if let Some(msg) = write_sub.next().await {
            let p = serde_json::to_vec(&serde_json::json!({"ok": true})).unwrap();
            nats.publish(msg.reply.unwrap(), p.into()).await.unwrap();
        }
        if let Some(msg) = output_sub.next().await {
            let full = format!("{output}__EXIT_0__\n");
            let p = serde_json::to_vec(&serde_json::json!({"output": full})).unwrap();
            nats.publish(msg.reply.unwrap(), p.into()).await.unwrap();
        }
    });
    captured
}

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

    let ext_base = format!("{prefix}.session.{session_id}.client.ext");

    // Spawn NATS mock responders before the agent runs.
    spawn_terminal_responders(nats.clone(), base, ext_base, "tid-001", cmd_output);
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
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn("Command completed successfully."));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_bash_tool_use(command));
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

/// The `sandbox_dir` passed to `WasmRuntimeBashTool::new` must appear in the
/// `terminal.create` NATS message payload as the working directory for the
/// bash terminal — verified by inspecting the raw JSON payload.
#[tokio::test]
async fn bash_sandbox_dir_passed_as_cwd_in_create_terminal_request() {
    let (_c, port) = start_nats().await;
    let nats = connect_nats(port).await;

    let dir = tempfile::TempDir::new().unwrap();
    let sandbox_path = dir.path().to_str().unwrap().to_string();

    let prefix = "acp.wasm";
    let session_id = "cwd-sess-1";
    let base = format!("{prefix}.session.{session_id}.client.terminal");
    let ext_base = format!("{prefix}.session.{session_id}.client.ext");

    let captured = spawn_terminal_responders_capture_create(
        nats.clone(),
        base,
        ext_base,
        "tid-cwd",
        "cwd-out\n",
    );
    tokio::time::sleep(Duration::from_millis(50)).await;

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages").body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn("done"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_bash_tool_use("echo cwd-test"));
    });

    // Pass the real tempdir as sandbox_dir.
    let bash_tool = trogon_acp_runner::wasm_bash_tool::WasmRuntimeBashTool::new(
        nats.clone(),
        prefix,
        session_id,
        dir.path().to_path_buf(),
        Duration::from_secs(10),
        trogon_acp_runner::session_store::mock::MemorySessionStore::new(),
    );
    let (dispatch_name, orig_name, client) = bash_tool.into_dispatch();

    let agent = AgentLoop {
        http_client: reqwest::Client::new(),
        proxy_url: "http://127.0.0.1:1".to_string(),
        anthropic_token: "test-token".to_string(),
        streaming_client: None,
        anthropic_base_url: Some(server.base_url()),
        anthropic_extra_headers: vec![],
        model: "claude-test".to_string(),
        max_iterations: 5,
        thinking_budget: None,
        tool_context: Arc::new(ToolContext {
            proxy_url: "http://127.0.0.1:1".to_string(),
            cwd: ".".to_string(),
            http_client: reqwest::Client::new(),
        }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![trogon_acp_runner::wasm_bash_tool::WasmRuntimeBashTool::<
            trogon_acp_runner::session_store::mock::MemorySessionStore,
        >::tool_def()],
        mcp_dispatch: vec![(dispatch_name, orig_name, client)],
        permission_checker: None,
        elicitation_provider: None,
    };

    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    agent
        .run_chat_streaming(vec![Message::user_text("run bash")], &[], None, tx, None)
        .await
        .expect("run_chat_streaming must succeed");

    let raw = captured
        .lock()
        .unwrap()
        .take()
        .expect("terminal.create must have been called");
    let payload_str = String::from_utf8_lossy(&raw);
    assert!(
        payload_str.contains(&sandbox_path),
        "CreateTerminalRequest payload must contain sandbox_dir path '{sandbox_path}'; got: {payload_str}"
    );
}

/// The second bash call within the same session must reuse the terminal created
/// during the first call — verified by asserting that `terminal.create` is
/// called exactly once across two sequential bash tool dispatches.
#[tokio::test]
async fn bash_terminal_reuse_skips_create_on_second_call() {
    let (_c, port) = start_nats().await;
    let nats = connect_nats(port).await;

    let prefix = "acp.wasm";
    let session_id = "reuse-sess-1";
    let base = format!("{prefix}.session.{session_id}.client.terminal");
    let ext_base = format!("{prefix}.session.{session_id}.client.ext");

    let create_count =
        spawn_two_bash_responders(nats.clone(), base, ext_base, "tid-reuse");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 3 mock LLM responses:
    //   call 1 (no tool_result)          → bash_001 tool_use
    //   call 2 (has bash_001 tool_result, body contains "bash_001") → bash_002 tool_use
    //   call 3 (body contains "bash_002") → end_turn
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages").body_contains("bash_002");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn("both commands done"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages").body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_bash_tool_use_with_id("echo second", "bash_002"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_bash_tool_use_with_id("echo first", "bash_001"));
    });

    let agent = make_agent(&server.base_url(), nats, prefix, session_id);

    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    agent
        .run_chat_streaming(
            vec![Message::user_text("run two commands")],
            &[],
            None,
            tx,
            None,
        )
        .await
        .expect("run_chat_streaming must succeed");

    assert_eq!(
        create_count.load(Ordering::SeqCst),
        1,
        "terminal.create must be called exactly once — second bash call must reuse the terminal"
    );
}

/// Like `spawn_terminal_responders` but lets the caller choose the exit code
/// embedded in the `__EXIT_N__` demarcation marker.
fn spawn_terminal_responders_with_exit_code(
    nats: async_nats::Client,
    base: String,
    ext_base: String,
    terminal_id: &'static str,
    output: &'static str,
    exit_code: u8,
) {
    tokio::spawn(async move {
        let mut create_sub = nats.subscribe(format!("{base}.create")).await.unwrap();
        let mut output_sub = nats.subscribe(format!("{base}.output")).await.unwrap();
        let mut write_sub = nats
            .subscribe(format!("{ext_base}.terminal.write_stdin"))
            .await
            .unwrap();

        if let Some(msg) = create_sub.next().await {
            let resp = CreateTerminalResponse::new(TerminalId::new(terminal_id));
            nats.publish(msg.reply.unwrap(), serde_json::to_vec(&resp).unwrap().into())
                .await
                .unwrap();
        }
        if let Some(msg) = output_sub.next().await {
            nats.publish(
                msg.reply.unwrap(),
                serde_json::to_vec(&serde_json::json!({"output": ""})).unwrap().into(),
            )
            .await
            .unwrap();
        }
        if let Some(msg) = write_sub.next().await {
            nats.publish(
                msg.reply.unwrap(),
                serde_json::to_vec(&serde_json::json!({"ok": true})).unwrap().into(),
            )
            .await
            .unwrap();
        }
        if let Some(msg) = output_sub.next().await {
            let full = format!("{output}__EXIT_{exit_code}__\n");
            nats.publish(
                msg.reply.unwrap(),
                serde_json::to_vec(&serde_json::json!({"output": full})).unwrap().into(),
            )
            .await
            .unwrap();
        }
    });
}

/// A non-zero exit code in the `__EXIT_N__` demarcation marker must NOT stall
/// the bash tool — the agent loop completes and `ToolCallFinished` carries the
/// command output with the marker stripped.
///
/// Exercises the full NATS dispatch chain with `__EXIT_1__` to verify that
/// non-zero exit codes are handled identically to `__EXIT_0__`.
#[tokio::test]
async fn bash_nonzero_exit_code_in_marker_completes_and_strips_marker() {
    let (_c, port) = start_nats().await;
    let nats = connect_nats(port).await;

    let prefix = "acp.wasm";
    let session_id = "exit1-sess";
    let command = "cat /nonexistent";
    let cmd_output = "cat: /nonexistent: No such file or directory\n";
    let base = format!("{prefix}.session.{session_id}.client.terminal");
    let ext_base = format!("{prefix}.session.{session_id}.client.ext");

    spawn_terminal_responders_with_exit_code(
        nats.clone(),
        base,
        ext_base,
        "tid-exit1",
        cmd_output,
        1,
    );
    tokio::time::sleep(Duration::from_millis(50)).await;

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/messages").body_contains("tool_result");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn("command failed as expected"));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_bash_tool_use(command));
    });

    let agent = make_agent(&server.base_url(), nats, prefix, session_id);
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let result = agent
        .run_chat_streaming(
            vec![Message::user_text("run failing command")],
            &[],
            None,
            tx,
            None,
        )
        .await;

    assert!(
        result.is_ok(),
        "run_chat_streaming must succeed even with non-zero exit; got: {:?}",
        result.err()
    );

    let mut events = vec![];
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }

    let finished = events
        .iter()
        .find(|e| matches!(e, AgentEvent::ToolCallFinished { .. }))
        .expect("expected ToolCallFinished event");
    if let AgentEvent::ToolCallFinished { output, .. } = finished {
        assert!(
            output.contains("No such file"),
            "ToolCallFinished must contain the command output; got: {output}"
        );
        assert!(
            !output.contains("__EXIT_"),
            "ToolCallFinished output must NOT contain the __EXIT_ marker; got: {output}"
        );
    }
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
            .header("Content-Type", "text/event-stream")
            .body(sse_end_turn("Handled the error."));
    });
    server.mock(|when, then| {
        when.method(POST).path("/messages");
        then.status(200)
            .header("Content-Type", "text/event-stream")
            .body(sse_bash_tool_use("echo hi"));
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
