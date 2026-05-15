//! Integration tests for WasmRuntimeBashTool — requires Docker (testcontainers starts NATS).
//!
//! The tool communicates over three NATS request-reply subjects:
//!   {prefix}.session.{id}.client.terminal.create      → CreateTerminalResponse JSON
//!   {prefix}.session.{id}.client.terminal.output      → {"output":"..."} JSON
//!   {prefix}.session.{id}.client.ext.terminal.write_stdin → any reply (ack)
//!
//! The output mock serves "" on the baseline poll (before write_stdin) and the
//! configured sequence on every subsequent poll, using an AtomicBool synced via
//! the write_stdin handler.
//!
//! Run with:
//!   cargo test -p trogon-runner-tools --test wasm_bash_integration

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use trogon_mcp::McpCallTool as _;
use trogon_runner_tools::session_store::mock::MemorySessionStore;
use trogon_runner_tools::session_store::{SessionState, SessionStore as _};
use trogon_runner_tools::wasm_bash_tool::WasmRuntimeBashTool;

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

fn sandbox() -> PathBuf {
    std::env::temp_dir()
}

fn make_tool(
    nats: async_nats::Client,
    prefix: &str,
    session_id: &str,
    timeout: Duration,
) -> WasmRuntimeBashTool<MemorySessionStore> {
    WasmRuntimeBashTool::new(
        nats,
        prefix,
        session_id,
        sandbox(),
        timeout,
        MemorySessionStore::new(),
    )
}

/// Spawn a mock wasm-runtime that handles the three NATS subjects.
///
/// `outputs_after_write` drives what `terminal.output` returns **after** the
/// `write_stdin` call. Before `write_stdin`, the output handler always returns
/// `""` (the baseline). An `AtomicBool` set by the `write_stdin` handler
/// synchronises the two without any sleeps.
async fn spawn_mock_runtime(
    client: async_nats::Client,
    prefix: &str,
    session: &str,
    terminal_id: &str,
    outputs_after_write: Vec<String>,
) {
    let term_base = format!("{prefix}.session.{session}.client.terminal");
    let ext_base = format!("{prefix}.session.{session}.client.ext");
    let tid = terminal_id.to_string();

    let mut create_sub = client.subscribe(format!("{term_base}.create")).await.unwrap();
    let mut output_sub = client.subscribe(format!("{term_base}.output")).await.unwrap();
    let mut stdin_sub = client
        .subscribe(format!("{ext_base}.terminal.write_stdin"))
        .await
        .unwrap();

    // Shared flag: set to true once write_stdin is acknowledged.
    let write_done = Arc::new(AtomicBool::new(false));

    // .create handler
    let c = client.clone();
    tokio::spawn(async move {
        if let Some(msg) = create_sub.next().await {
            if let Some(reply) = msg.reply {
                let resp = serde_json::json!({ "terminalId": tid });
                c.publish(reply, serde_json::to_vec(&resp).unwrap().into())
                    .await
                    .unwrap();
            }
        }
    });

    // write_stdin handler: set flag then ack
    let wd = write_done.clone();
    let c = client.clone();
    tokio::spawn(async move {
        if let Some(msg) = stdin_sub.next().await {
            wd.store(true, Ordering::Release);
            if let Some(reply) = msg.reply {
                c.publish(reply, "ok".into()).await.unwrap();
            }
        }
    });

    // .output handler: "" before write, sequence after write
    let wd = write_done.clone();
    let c = client.clone();
    tokio::spawn(async move {
        let mut idx = 0usize;
        while let Some(msg) = output_sub.next().await {
            if let Some(reply) = msg.reply {
                let output = if wd.load(Ordering::Acquire) {
                    let v = outputs_after_write
                        .get(idx)
                        .cloned()
                        .unwrap_or_else(|| outputs_after_write.last().cloned().unwrap_or_default());
                    idx += 1;
                    v
                } else {
                    String::new()
                };
                let resp = serde_json::json!({ "output": output });
                c.publish(reply, serde_json::to_vec(&resp).unwrap().into())
                    .await
                    .unwrap();
            }
        }
    });

    tokio::task::yield_now().await;
}

// ── Constructor / into_dispatch ───────────────────────────────────────────────

#[tokio::test]
async fn into_dispatch_returns_bash_names() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;

    let tool = make_tool(client, "t", "s1", Duration::from_secs(5));
    let (server_name, tool_name, _handler) = tool.into_dispatch();

    assert_eq!(server_name, "bash");
    assert_eq!(tool_name, "bash");
}

#[tokio::test]
async fn into_dispatch_produces_valid_arc() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;

    let tool = make_tool(client, "t", "s1", Duration::from_secs(5));
    let (_sn, _tn, handler) = tool.into_dispatch();

    assert_eq!(Arc::strong_count(&handler), 1);
}

// ── call_tool — missing argument ──────────────────────────────────────────────

#[tokio::test]
async fn call_tool_errors_on_missing_command() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;

    let tool = make_tool(client, "t", "s1", Duration::from_secs(5));
    let result = tool.call_tool("bash", &serde_json::json!({})).await;

    assert!(result.is_err());
    assert!(
        result.unwrap_err().contains("command"),
        "error should mention 'command'"
    );
}

#[tokio::test]
async fn call_tool_errors_on_null_command() {
    let (_container, port) = start_nats().await;
    let client = nats_client(port).await;

    let tool = make_tool(client, "t", "s1", Duration::from_secs(5));
    let result = tool
        .call_tool("bash", &serde_json::json!({ "command": null }))
        .await;

    assert!(result.is_err());
}

// ── call_tool — happy path ────────────────────────────────────────────────────

#[tokio::test]
async fn call_tool_creates_terminal_and_returns_output() {
    let (_container, port) = start_nats().await;
    let tool_client = nats_client(port).await;
    let mock_client = nats_client(port).await;

    spawn_mock_runtime(
        mock_client,
        "wp",
        "sess1",
        "term-abc",
        vec!["hello world\n__EXIT_0__\n".to_string()],
    )
    .await;

    let tool = make_tool(tool_client, "wp", "sess1", Duration::from_secs(5));
    let result = tool
        .call_tool("bash", &serde_json::json!({ "command": "echo hello world" }))
        .await;

    assert!(result.is_ok(), "expected Ok, got {:?}", result.unwrap_err());
    assert!(result.unwrap().contains("hello world"));
}

/// The exit marker is stripped from the returned output.
#[tokio::test]
async fn call_tool_strips_exit_marker_from_output() {
    let (_container, port) = start_nats().await;
    let tool_client = nats_client(port).await;
    let mock_client = nats_client(port).await;

    spawn_mock_runtime(
        mock_client,
        "wp",
        "sess2",
        "term-def",
        vec!["line1\nline2\n__EXIT_0__\n".to_string()],
    )
    .await;

    let tool = make_tool(tool_client, "wp", "sess2", Duration::from_secs(5));
    let result = tool
        .call_tool("bash", &serde_json::json!({ "command": "cat file" }))
        .await
        .unwrap();

    assert!(!result.contains("__EXIT_"), "marker must be stripped, got: {result}");
    assert!(result.contains("line1"), "got: {result}");
    assert!(result.contains("line2"), "got: {result}");
}

/// Non-zero exit codes are accepted — the marker is stripped regardless of code.
#[tokio::test]
async fn call_tool_accepts_nonzero_exit_code() {
    let (_container, port) = start_nats().await;
    let tool_client = nats_client(port).await;
    let mock_client = nats_client(port).await;

    spawn_mock_runtime(
        mock_client,
        "wp",
        "sess3",
        "term-ghi",
        vec!["error: file not found\n__EXIT_1__\n".to_string()],
    )
    .await;

    let tool = make_tool(tool_client, "wp", "sess3", Duration::from_secs(5));
    let result = tool
        .call_tool("bash", &serde_json::json!({ "command": "cat missing" }))
        .await;

    assert!(result.is_ok(), "non-zero exit should still be Ok: {:?}", result.unwrap_err());
    assert!(result.unwrap().contains("error: file not found"));
}

/// Terminal ID is persisted in session state after the first call.
#[tokio::test]
async fn call_tool_persists_terminal_id_in_session_state() {
    let (_container, port) = start_nats().await;
    let tool_client = nats_client(port).await;
    let mock_client = nats_client(port).await;

    let store = MemorySessionStore::new();

    spawn_mock_runtime(
        mock_client,
        "wp",
        "sess4",
        "term-persist",
        vec!["output\n__EXIT_0__\n".to_string()],
    )
    .await;

    let tool = WasmRuntimeBashTool::new(
        tool_client,
        "wp",
        "sess4",
        sandbox(),
        Duration::from_secs(5),
        store.clone(),
    );

    tool.call_tool("bash", &serde_json::json!({ "command": "pwd" }))
        .await
        .unwrap();

    let state = store.load("sess4").await.expect("state must be loadable");
    assert_eq!(
        state.terminal_id.as_deref(),
        Some("term-persist"),
        "terminal_id must be saved after first call"
    );
}

/// When the session already has a terminal_id, the `.create` subject is skipped.
#[tokio::test]
async fn call_tool_reuses_existing_terminal_id() {
    let (_container, port) = start_nats().await;
    let tool_client = nats_client(port).await;
    let mock_client = nats_client(port).await;

    let store = MemorySessionStore::new();
    let mut state = SessionState::default();
    state.terminal_id = Some("existing-term".to_string());
    store.save("sess5", &state).await.unwrap();

    let term_base = "wp.session.sess5.client.terminal";
    let ext_base = "wp.session.sess5.client.ext";

    // No `.create` subscriber — if the tool tries to create, it will fail immediately.
    let mut output_sub = mock_client.subscribe(format!("{term_base}.output")).await.unwrap();
    let mut stdin_sub = mock_client
        .subscribe(format!("{ext_base}.terminal.write_stdin"))
        .await
        .unwrap();

    let write_done = Arc::new(AtomicBool::new(false));

    let wd = write_done.clone();
    let mc = mock_client.clone();
    tokio::spawn(async move {
        if let Some(msg) = stdin_sub.next().await {
            wd.store(true, Ordering::Release);
            if let Some(reply) = msg.reply {
                mc.publish(reply, "ack".into()).await.unwrap();
            }
        }
    });

    let wd = write_done.clone();
    let mc = mock_client.clone();
    tokio::spawn(async move {
        while let Some(msg) = output_sub.next().await {
            if let Some(reply) = msg.reply {
                let output = if wd.load(Ordering::Acquire) {
                    "reused\n__EXIT_0__\n".to_string()
                } else {
                    String::new()
                };
                let resp = serde_json::json!({ "output": output });
                mc.publish(reply, serde_json::to_vec(&resp).unwrap().into())
                    .await
                    .unwrap();
            }
        }
    });

    tokio::task::yield_now().await;

    let tool = WasmRuntimeBashTool::new(
        tool_client,
        "wp",
        "sess5",
        sandbox(),
        Duration::from_secs(5),
        store,
    );

    let result = tool
        .call_tool("bash", &serde_json::json!({ "command": "echo reused" }))
        .await;

    assert!(result.is_ok(), "expected Ok: {:?}", result.unwrap_err());
    assert!(result.unwrap().contains("reused"));
}

// ── call_tool — NATS prefix ───────────────────────────────────────────────────

#[tokio::test]
async fn call_tool_uses_correct_nats_prefix() {
    let (_container, port) = start_nats().await;
    let tool_client = nats_client(port).await;
    let mock_client = nats_client(port).await;

    spawn_mock_runtime(
        mock_client,
        "team.prod",
        "s6",
        "t6",
        vec!["out\n__EXIT_0__\n".to_string()],
    )
    .await;

    let tool = make_tool(tool_client, "team.prod", "s6", Duration::from_secs(5));
    let result = tool
        .call_tool("bash", &serde_json::json!({ "command": "true" }))
        .await;

    assert!(result.is_ok(), "prefix routing failed: {:?}", result.unwrap_err());
}

// ── call_tool — timeout ───────────────────────────────────────────────────────

/// When the marker never arrives, `call_tool` returns an error mentioning "timeout".
#[tokio::test]
async fn call_tool_times_out_when_marker_never_arrives() {
    let (_container, port) = start_nats().await;
    let tool_client = nats_client(port).await;
    let mock_client = nats_client(port).await;

    let term_base = "wp.session.stimeout.client.terminal";
    let ext_base = "wp.session.stimeout.client.ext";

    let mut create_sub = mock_client.subscribe(format!("{term_base}.create")).await.unwrap();
    let mut output_sub = mock_client.subscribe(format!("{term_base}.output")).await.unwrap();
    let mut stdin_sub = mock_client
        .subscribe(format!("{ext_base}.terminal.write_stdin"))
        .await
        .unwrap();

    let write_done = Arc::new(AtomicBool::new(false));

    let mc = mock_client.clone();
    tokio::spawn(async move {
        if let Some(msg) = create_sub.next().await {
            if let Some(reply) = msg.reply {
                let resp = serde_json::json!({ "terminalId": "t-timeout" });
                mc.publish(reply, serde_json::to_vec(&resp).unwrap().into())
                    .await
                    .unwrap();
            }
        }
    });

    let wd = write_done.clone();
    let mc = mock_client.clone();
    tokio::spawn(async move {
        if let Some(msg) = stdin_sub.next().await {
            wd.store(true, Ordering::Release);
            if let Some(reply) = msg.reply {
                mc.publish(reply, "ack".into()).await.unwrap();
            }
        }
    });

    let wd = write_done.clone();
    let mc = mock_client.clone();
    tokio::spawn(async move {
        while let Some(msg) = output_sub.next().await {
            if let Some(reply) = msg.reply {
                // Never produce an exit marker — always return non-empty output after write.
                let output = if wd.load(Ordering::Acquire) {
                    "still running...".to_string()
                } else {
                    String::new()
                };
                let resp = serde_json::json!({ "output": output });
                mc.publish(reply, serde_json::to_vec(&resp).unwrap().into())
                    .await
                    .unwrap();
            }
        }
    });

    tokio::task::yield_now().await;

    let tool = make_tool(tool_client, "wp", "stimeout", Duration::from_millis(250));
    let result = tool
        .call_tool("bash", &serde_json::json!({ "command": "sleep 999" }))
        .await;

    assert!(result.is_err(), "expected timeout error");
    assert!(result.unwrap_err().contains("timeout"));
}

/// Partial output collected before timeout is included in the error.
#[tokio::test]
async fn call_tool_timeout_includes_partial_output() {
    let (_container, port) = start_nats().await;
    let tool_client = nats_client(port).await;
    let mock_client = nats_client(port).await;

    let term_base = "wp.session.spartial.client.terminal";
    let ext_base = "wp.session.spartial.client.ext";

    let mut create_sub = mock_client.subscribe(format!("{term_base}.create")).await.unwrap();
    let mut output_sub = mock_client.subscribe(format!("{term_base}.output")).await.unwrap();
    let mut stdin_sub = mock_client
        .subscribe(format!("{ext_base}.terminal.write_stdin"))
        .await
        .unwrap();

    let write_done = Arc::new(AtomicBool::new(false));

    let mc = mock_client.clone();
    tokio::spawn(async move {
        if let Some(msg) = create_sub.next().await {
            if let Some(reply) = msg.reply {
                let resp = serde_json::json!({ "terminalId": "t-partial" });
                mc.publish(reply, serde_json::to_vec(&resp).unwrap().into())
                    .await
                    .unwrap();
            }
        }
    });

    let wd = write_done.clone();
    let mc = mock_client.clone();
    tokio::spawn(async move {
        if let Some(msg) = stdin_sub.next().await {
            wd.store(true, Ordering::Release);
            if let Some(reply) = msg.reply {
                mc.publish(reply, "ack".into()).await.unwrap();
            }
        }
    });

    let wd = write_done.clone();
    let mc = mock_client.clone();
    tokio::spawn(async move {
        while let Some(msg) = output_sub.next().await {
            if let Some(reply) = msg.reply {
                // Baseline returns ""; after write returns growing output with no marker.
                let output = if wd.load(Ordering::Acquire) {
                    "partial output line\n".to_string()
                } else {
                    String::new()
                };
                let resp = serde_json::json!({ "output": output });
                mc.publish(reply, serde_json::to_vec(&resp).unwrap().into())
                    .await
                    .unwrap();
            }
        }
    });

    tokio::task::yield_now().await;

    let tool = make_tool(tool_client, "wp", "spartial", Duration::from_millis(350));
    let err = tool
        .call_tool("bash", &serde_json::json!({ "command": "long-cmd" }))
        .await
        .unwrap_err();

    assert!(err.contains("timeout"), "must mention timeout: {err}");
    assert!(err.contains("partial output line"), "must include partial output: {err}");
}
