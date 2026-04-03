/// Integration tests for `dispatcher::run` — requires a real NATS server.
///
/// Set `NATS_TEST_URL=nats://localhost:4222` to run these tests.
/// Without it every test is skipped via early return.
///
/// Subject format: `{prefix}.session.{session_id}.client.{method_suffix}`
use agent_client_protocol::{
    ContentBlock, ContentChunk, CreateTerminalRequest, ReadTextFileRequest, ReleaseTerminalRequest,
    SessionId, SessionNotification, SessionUpdate, TerminalId, WriteTextFileRequest,
};
use futures::future::join_all;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;
use tempfile::TempDir;
use trogon_wasm_runtime::{Config, WasmRuntime};

// ── helpers ──────────────────────────────────────────────────────────────────

fn nats_url() -> Option<String> {
    std::env::var("NATS_TEST_URL").ok()
}

fn dispatcher_config(session_root: PathBuf) -> Config {
    Config {
        wasm_only: false,
        auto_allow_permissions: true,
        wasm_fuel_limit: 1_000_000_000,
        wasm_host_call_limit: 10_000,
        output_byte_limit: 1024 * 1024,
        wasm_timeout_secs: None,
        wasm_memory_limit_bytes: None,
        module_cache_dir: None,
        wasm_allow_network: false,
        acp_prefix: "disp-test".to_string(),
        wasm_max_concurrent_tasks: 32,
        session_idle_timeout_secs: 3600,
        wasm_max_module_size_bytes: 100 * 1024 * 1024,
        wait_for_exit_timeout_secs: 30,
        session_root,
    }
}

/// Unique prefix per test to avoid subject collisions between concurrent test runs.
fn test_prefix(tag: &str) -> String {
    format!("disp-test-{tag}")
}

/// Publish a request and wait for a reply (timeout 5s). Returns raw reply bytes.
async fn nats_request(
    nats: &async_nats::Client,
    subject: String,
    payload: impl Into<bytes::Bytes>,
) -> bytes::Bytes {
    let msg = tokio::time::timeout(
        Duration::from_secs(5),
        nats.request(subject, payload.into()),
    )
    .await
    .expect("request timed out")
    .expect("NATS request failed");
    msg.payload
}

/// Publish fire-and-forget (no reply expected).
async fn nats_publish(
    nats: &async_nats::Client,
    subject: String,
    payload: impl Into<bytes::Bytes>,
) {
    nats.publish(subject, payload.into())
        .await
        .expect("publish failed");
    nats.flush().await.expect("flush failed");
}

/// Make a trivial .wasm that exits with code 0.
fn make_exit0_wasm(dir: &std::path::Path) -> PathBuf {
    let wat = r#"(module
  (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
  (memory 1) (export "memory" (memory 0))
  (func (export "_start") (call $exit (i32.const 0)))
)"#;
    let path = dir.join("exit0.wasm");
    let wasm = wat::parse_str(wat).expect("WAT parse");
    std::fs::write(&path, wasm).expect("write wasm");
    path
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Dispatcher routes `terminal.create` → `WasmRuntime::handle_create_terminal`
/// and publishes a JSON reply with a `terminal_id`. Then releases it.
#[tokio::test]
async fn dispatcher_terminal_create_and_release() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping dispatcher_terminal_create_and_release");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let wasm_path = make_exit0_wasm(tmp.path());
    let prefix = test_prefix("create");
    let session_id = "sess-create";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Send terminal.create
            let req = CreateTerminalRequest::new(
                SessionId::from(session_id),
                wasm_path.to_str().unwrap(),
            );
            let subject = format!("{prefix}.session.{session_id}.client.terminal.create");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&req).unwrap()).await;

            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "unexpected error: {resp}");
            let tid = resp["terminalId"].as_str().expect("terminal_id missing");
            assert!(!tid.is_empty());

            // Send terminal.release
            let rel = ReleaseTerminalRequest::new(
                SessionId::from(session_id),
                TerminalId::new(tid.to_string()),
            );
            let subject = format!("{prefix}.session.{session_id}.client.terminal.release");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&rel).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "release error: {resp}");

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// Invalid JSON payload returns a -32600 error reply.
#[tokio::test]
async fn dispatcher_invalid_json_returns_error() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping dispatcher_invalid_json_returns_error");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("invalid-json");
    let session_id = "sess-invalid";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            let subject = format!("{prefix}.session.{session_id}.client.terminal.create");
            let reply = nats_request(&nats, subject, bytes::Bytes::from_static(b"not-valid-json")).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();

            let code = resp["error"]["code"].as_i64().expect("error.code missing");
            assert_eq!(code, -32600, "expected -32600 for invalid JSON, got {resp}");

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// Message with no reply subject (fire-and-forget) does not crash the dispatcher.
#[tokio::test]
async fn dispatcher_no_reply_subject_does_not_crash() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!(
                "NATS_TEST_URL not set — skipping dispatcher_no_reply_subject_does_not_crash"
            );
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("no-reply");
    let session_id = "sess-noreply";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Publish terminal.create with no reply subject (nats.publish, not nats.request).
            let req = CreateTerminalRequest::new(
                SessionId::from(session_id),
                "/nonexistent/path.wasm",
            );
            let subject = format!("{prefix}.session.{session_id}.client.terminal.create");
            nats_publish(&nats, subject, serde_json::to_vec(&req).unwrap()).await;

            tokio::time::sleep(Duration::from_millis(200)).await;

            // Dispatcher must still be alive — send a valid request and get a reply.
            let subject = format!("{prefix}.session.{session_id}.client.ext.runtime.list_sessions");
            let reply = nats_request(&nats, subject, bytes::Bytes::from_static(b"{}")).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("sessions").is_some(), "dispatcher crashed: {resp}");

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// `SessionUpdate` (fire-and-forget) does not crash the dispatcher.
#[tokio::test]
async fn dispatcher_session_update_fire_and_forget() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!(
                "NATS_TEST_URL not set — skipping dispatcher_session_update_fire_and_forget"
            );
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("session-update");
    let session_id = "sess-update";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // SessionNotification is fire-and-forget.
            let notif = SessionNotification::new(
                session_id,
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("ping"))),
            );
            let subject = format!("{prefix}.session.{session_id}.client.session.update");
            nats_publish(&nats, subject, serde_json::to_vec(&notif).unwrap()).await;

            tokio::time::sleep(Duration::from_millis(200)).await;

            // Dispatcher must still be alive.
            let subject = format!("{prefix}.session.{session_id}.client.ext.runtime.list_sessions");
            let reply = nats_request(&nats, subject, bytes::Bytes::from_static(b"{}")).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("sessions").is_some(), "dispatcher crashed: {resp}");

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// `ext.runtime.list_sessions` returns `{ "sessions": [...] }` containing the active session.
#[tokio::test]
async fn dispatcher_ext_list_sessions() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping dispatcher_ext_list_sessions");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let wasm_path = make_exit0_wasm(tmp.path());
    let prefix = test_prefix("list-sess");
    let session_id = "sess-list";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Create a terminal so the session appears in the list.
            let req = CreateTerminalRequest::new(
                SessionId::from(session_id),
                wasm_path.to_str().unwrap(),
            );
            let subject = format!("{prefix}.session.{session_id}.client.terminal.create");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "create error: {resp}");

            // list_sessions
            let subject = format!("{prefix}.session.{session_id}.client.ext.runtime.list_sessions");
            let reply = nats_request(&nats, subject, bytes::Bytes::from_static(b"{}")).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            let sessions = resp["sessions"].as_array().expect("sessions array");
            assert!(
                sessions.iter().any(|s| s.as_str() == Some(session_id)),
                "session {session_id} not in list: {sessions:?}"
            );

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// `ext.runtime.list_terminals` returns `{ "terminals": [...] }` containing the active terminal.
#[tokio::test]
async fn dispatcher_ext_list_terminals() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping dispatcher_ext_list_terminals");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let wasm_path = make_exit0_wasm(tmp.path());
    let prefix = test_prefix("list-terms");
    let session_id = "sess-terms";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Create a terminal.
            let req = CreateTerminalRequest::new(
                SessionId::from(session_id),
                wasm_path.to_str().unwrap(),
            );
            let subject = format!("{prefix}.session.{session_id}.client.terminal.create");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            let tid = resp["terminalId"].as_str().expect("terminal_id");

            // list_terminals
            let subject = format!("{prefix}.session.{session_id}.client.ext.runtime.list_terminals");
            let reply = nats_request(&nats, subject, bytes::Bytes::from_static(b"{}")).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            let terminals = resp["terminals"].as_array().expect("terminals array");
            // list_terminals is built manually in dispatcher with snake_case keys.
            assert!(
                terminals.iter().any(|t| t["terminal_id"].as_str() == Some(tid)),
                "terminal {tid} not in list: {terminals:?}"
            );

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// `ext.session.prompt_response` returns `-32601` (not supported by this runtime).
#[tokio::test]
async fn dispatcher_ext_prompt_response_not_supported() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!(
                "NATS_TEST_URL not set — skipping dispatcher_ext_prompt_response_not_supported"
            );
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("prompt-resp");
    let session_id = "sess-prompt";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            let subject = format!("{prefix}.session.{session_id}.client.ext.session.prompt_response");
            let reply = nats_request(&nats, subject, bytes::Bytes::from_static(b"{}")).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            let code = resp["error"]["code"].as_i64().expect("error.code");
            assert_eq!(code, -32601, "expected -32601, got {resp}");

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// Unknown ext method returns `-32601`.
#[tokio::test]
async fn dispatcher_unknown_ext_method_returns_not_found() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!(
                "NATS_TEST_URL not set — skipping dispatcher_unknown_ext_method_returns_not_found"
            );
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("unknown-ext");
    let session_id = "sess-unknown";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            let subject = format!("{prefix}.session.{session_id}.client.ext.runtime.no_such_method");
            let reply = nats_request(&nats, subject, bytes::Bytes::from_static(b"{}")).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            let code = resp["error"]["code"].as_i64().expect("error.code");
            assert_eq!(code, -32601, "expected -32601, got {resp}");

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// `terminal.write_stdin` ext method: unknown terminal returns an error reply, no crash.
#[tokio::test]
async fn dispatcher_ext_write_stdin_unknown_terminal() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping dispatcher_ext_write_stdin_unknown_terminal");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("write-stdin");
    let session_id = "sess-stdin";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            let payload = serde_json::json!({
                "terminal_id": "nonexistent-terminal-id",
                "data": [104, 101, 108, 108, 111]
            });
            let subject = format!("{prefix}.session.{session_id}.client.ext.terminal.write_stdin");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&payload).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(
                resp.get("error").is_some(),
                "expected error for unknown terminal, got {resp}"
            );

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// `terminal.close_stdin` ext method: unknown terminal returns an error reply, no crash.
#[tokio::test]
async fn dispatcher_ext_close_stdin_unknown_terminal() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping dispatcher_ext_close_stdin_unknown_terminal");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("close-stdin");
    let session_id = "sess-close-stdin";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            let payload = serde_json::json!({ "terminal_id": "nonexistent-terminal-id" });
            let subject = format!("{prefix}.session.{session_id}.client.ext.terminal.close_stdin");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&payload).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(
                resp.get("error").is_some(),
                "expected error for unknown terminal, got {resp}"
            );

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// Shutdown signal causes dispatcher to drain and exit cleanly within 5 seconds.
#[tokio::test]
async fn dispatcher_shutdown_drains_and_exits() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping dispatcher_shutdown_drains_and_exits");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("shutdown");

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let handle = tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            let _ = shutdown_tx.send(true);

            tokio::time::timeout(Duration::from_secs(5), handle)
                .await
                .expect("dispatcher did not exit within 5s after shutdown signal")
                .expect("dispatcher task panicked");
        })
        .await;
}

/// `terminal.output` dispatcher route returns buffered output for a native terminal.
#[tokio::test]
async fn dispatcher_terminal_output_returns_output() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping dispatcher_terminal_output_returns_output");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("term-output");
    let session_id = "sess-output";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Create a native terminal: `echo hello`.
            let req = CreateTerminalRequest::new(session_id, "/bin/echo")
                .args(vec!["hello".to_string()]);
            let subject = format!("{prefix}.session.{session_id}.client.terminal.create");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "create error: {resp}");
            let tid = resp["terminalId"].as_str().expect("terminalId");

            // Give the process time to run and produce output.
            tokio::time::sleep(Duration::from_millis(300)).await;

            // Fetch terminal output via dispatcher.
            let out_req = serde_json::json!({ "sessionId": session_id, "terminalId": tid });
            let subject = format!("{prefix}.session.{session_id}.client.terminal.output");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&out_req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "output error: {resp}");
            let output = resp["output"].as_str().expect("output field");
            assert!(output.contains("hello"), "expected 'hello' in output, got: {output:?}");

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// `terminal.kill` dispatcher route stops a long-running native process without error.
#[tokio::test]
async fn dispatcher_terminal_kill_stops_process() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping dispatcher_terminal_kill_stops_process");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("term-kill");
    let session_id = "sess-kill";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Create a long-running native terminal: `sleep 30`.
            let req = CreateTerminalRequest::new(session_id, "/bin/sleep")
                .args(vec!["30".to_string()]);
            let subject = format!("{prefix}.session.{session_id}.client.terminal.create");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "create error: {resp}");
            let tid = resp["terminalId"].as_str().expect("terminalId");

            // Kill it via dispatcher.
            let kill_req = serde_json::json!({ "sessionId": session_id, "terminalId": tid });
            let subject = format!("{prefix}.session.{session_id}.client.terminal.kill");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&kill_req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "kill error: {resp}");

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// `terminal.wait_for_exit` dispatcher route returns an exit status for a completed terminal.
#[tokio::test]
async fn dispatcher_terminal_wait_for_exit_returns_status() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!(
                "NATS_TEST_URL not set — skipping dispatcher_terminal_wait_for_exit_returns_status"
            );
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let wasm_path = make_exit0_wasm(tmp.path());
    let prefix = test_prefix("wait-exit");
    let session_id = "sess-wait";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Create a WASM terminal that exits immediately with code 0.
            let req = CreateTerminalRequest::new(
                SessionId::from(session_id),
                wasm_path.to_str().unwrap(),
            );
            let subject = format!("{prefix}.session.{session_id}.client.terminal.create");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "create error: {resp}");
            let tid = resp["terminalId"].as_str().expect("terminalId");

            // Wait for it to complete.
            tokio::time::sleep(Duration::from_millis(500)).await;

            // wait_for_exit via dispatcher.
            // WaitForTerminalExitResponse flattens TerminalExitStatus, so exitCode/signal
            // appear at the top level of the response JSON.
            let wait_req = serde_json::json!({ "sessionId": session_id, "terminalId": tid });
            let subject = format!("{prefix}.session.{session_id}.client.terminal.wait_for_exit");
            let reply =
                nats_request(&nats, subject, serde_json::to_vec(&wait_req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "wait_for_exit error: {resp}");
            assert!(
                resp.get("exitCode").is_some() || resp.get("signal").is_some(),
                "expected exitCode or signal in response: {resp}"
            );

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// `session.request_permission` dispatcher route auto-selects first option when
/// `auto_allow_permissions` is true.
#[tokio::test]
async fn dispatcher_session_request_permission_auto_allow() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!(
                "NATS_TEST_URL not set — skipping dispatcher_session_request_permission_auto_allow"
            );
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("req-perm");
    let session_id = "sess-perm";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Build a RequestPermissionRequest as raw JSON.
            // ToolCallUpdate: { toolCallId: "...", ...ToolCallUpdateFields (all optional) }
            // PermissionOption: { optionId: "...", name: "...", kind: "allow_once" }
            let req = serde_json::json!({
                "sessionId": session_id,
                "toolCall": {
                    "toolCallId": "call-1"
                },
                "options": [
                    {
                        "optionId": "allow",
                        "name": "Allow",
                        "kind": "allow_once"
                    }
                ]
            });
            let subject =
                format!("{prefix}.session.{session_id}.client.session.request_permission");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "request_permission error: {resp}");

            // auto_allow_permissions: true → first option selected.
            // RequestPermissionOutcome is internally tagged: { "outcome": "selected", "optionId": "allow" }
            let outcome = &resp["outcome"];
            assert_eq!(
                outcome["outcome"].as_str(),
                Some("selected"),
                "expected selected outcome, got {resp}"
            );
            assert_eq!(
                outcome["optionId"].as_str(),
                Some("allow"),
                "expected optionId 'allow', got {resp}"
            );

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// Full WASM execution via NATS: create terminal → wait for exit → read output — all via dispatcher.
#[tokio::test]
async fn dispatcher_wasm_end_to_end() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping dispatcher_wasm_end_to_end");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let wasm_path = make_exit0_wasm(tmp.path());
    let prefix = test_prefix("wasm-e2e");
    let session_id = "sess-e2e";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // 1. Create a WASM terminal via NATS.
            let req = CreateTerminalRequest::new(
                SessionId::from(session_id),
                wasm_path.to_str().unwrap(),
            );
            let subject = format!("{prefix}.session.{session_id}.client.terminal.create");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "create error: {resp}");
            let tid = resp["terminalId"].as_str().expect("terminalId").to_string();

            // 2. Wait for it to exit via NATS.
            let wait_req = serde_json::json!({ "sessionId": session_id, "terminalId": tid });
            let subject = format!("{prefix}.session.{session_id}.client.terminal.wait_for_exit");
            let reply =
                nats_request(&nats, subject, serde_json::to_vec(&wait_req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "wait_for_exit error: {resp}");
            assert!(
                resp.get("exitCode").is_some() || resp.get("signal").is_some(),
                "expected exit status in wait response: {resp}"
            );

            // 3. Fetch terminal output via NATS.
            let out_req = serde_json::json!({ "sessionId": session_id, "terminalId": tid });
            let subject = format!("{prefix}.session.{session_id}.client.terminal.output");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&out_req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "terminal output error: {resp}");
            assert!(resp.get("output").is_some(), "output field missing: {resp}");
            assert!(
                resp.get("truncated").is_some(),
                "truncated field missing: {resp}"
            );

            // 4. Release the terminal.
            let rel = ReleaseTerminalRequest::new(
                SessionId::from(session_id),
                TerminalId::new(tid.clone()),
            );
            let subject = format!("{prefix}.session.{session_id}.client.terminal.release");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&rel).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "release error: {resp}");

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// Sending many concurrent requests via NATS — all must receive a reply.
#[tokio::test]
async fn dispatcher_concurrent_requests() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping dispatcher_concurrent_requests");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("concurrent");

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Fire 10 list_sessions requests concurrently — all must get a reply.
            let futs: Vec<_> = (0..10)
                .map(|i| {
                    let nats = nats.clone();
                    let subject =
                        format!("{prefix}.session.sess-{i}.client.ext.runtime.list_sessions");
                    async move {
                        let reply =
                            nats_request(&nats, subject, bytes::Bytes::from_static(b"{}")).await;
                        let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
                        resp.get("sessions").is_some()
                    }
                })
                .collect();

            let results = join_all(futs).await;
            let all_ok = results.iter().all(|&ok| ok);
            assert!(all_ok, "not all concurrent requests returned a sessions list");

            // Also fire 5 fs.write requests concurrently.
            let write_futs: Vec<_> = (0..5)
                .map(|i| {
                    let nats = nats.clone();
                    let sid = format!("sess-concurrent-{i}");
                    let prefix = prefix.clone();
                    let req = WriteTextFileRequest::new(
                        SessionId::from(sid.clone()),
                        PathBuf::from(format!("/file{i}.txt")),
                        format!("content-{i}"),
                    );
                    let subject = format!("{prefix}.session.{sid}.client.fs.write_text_file");
                    async move {
                        let reply =
                            nats_request(&nats, subject, serde_json::to_vec(&req).unwrap()).await;
                        let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
                        resp.get("error").is_none()
                    }
                })
                .collect();

            let write_results = join_all(write_futs).await;
            let all_writes_ok = write_results.iter().all(|&ok| ok);
            assert!(all_writes_ok, "not all concurrent write requests succeeded");

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// Dispatcher shutdown with an in-flight WASM task: the task must complete
/// before the dispatcher returns (graceful drain).
#[tokio::test]
async fn dispatcher_shutdown_drains_in_flight_task() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!(
                "NATS_TEST_URL not set — skipping dispatcher_shutdown_drains_in_flight_task"
            );
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let wasm_path = make_exit0_wasm(tmp.path());
    let prefix = test_prefix("drain");
    let session_id = "sess-drain";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let handle = tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Send a terminal.create request WITHOUT awaiting the reply — it will
            // be in-flight when we send the shutdown signal.
            let req = CreateTerminalRequest::new(
                SessionId::from(session_id),
                wasm_path.to_str().unwrap(),
            );
            let subject = format!("{prefix}.session.{session_id}.client.terminal.create");
            // Publish without waiting for a reply so the task is dispatched but
            // we don't block here.
            nats_publish(&nats, subject, serde_json::to_vec(&req).unwrap()).await;

            // Signal shutdown immediately after dispatching the task.
            let _ = shutdown_tx.send(true);

            // Dispatcher must drain (wait for the in-flight dispatch task) and exit
            // within 10 seconds.
            tokio::time::timeout(Duration::from_secs(10), handle)
                .await
                .expect("dispatcher did not exit within 10s after shutdown with in-flight task")
                .expect("dispatcher task panicked");
        })
        .await;
}

/// `fs.write_text_file` + `fs.read_text_file` round-trip through the dispatcher.
#[tokio::test]
async fn dispatcher_fs_write_read_roundtrip() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping dispatcher_fs_write_read_roundtrip");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("fs-rw");
    let session_id = "sess-fs";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Write a file.
            let write_req = WriteTextFileRequest::new(
                SessionId::from(session_id),
                PathBuf::from("hello.txt"),
                "hello dispatcher".to_string(),
            );
            let subject = format!("{prefix}.session.{session_id}.client.fs.write_text_file");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&write_req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "write error: {resp}");

            // Read it back.
            let read_req = ReadTextFileRequest::new(
                SessionId::from(session_id),
                PathBuf::from("hello.txt"),
            );
            let subject = format!("{prefix}.session.{session_id}.client.fs.read_text_file");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&read_req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "read error: {resp}");
            assert_eq!(resp["content"].as_str(), Some("hello dispatcher"));

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// `terminal.write_stdin` happy path: write data to a live `cat` process via the
/// dispatcher and verify the output is echoed back.
#[tokio::test]
async fn dispatcher_ext_write_stdin_happy_path() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping dispatcher_ext_write_stdin_happy_path");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("write-stdin-ok");
    let session_id = "sess-stdin-ok";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Spawn `cat` — it echoes everything written to stdin back to stdout.
            let req = CreateTerminalRequest::new(session_id, "/bin/cat");
            let subject = format!("{prefix}.session.{session_id}.client.terminal.create");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "create error: {resp}");
            let tid = resp["terminalId"].as_str().expect("terminalId").to_string();

            // Write "hello\n" as a byte array.
            let payload = serde_json::json!({
                "terminal_id": tid,
                "data": b"hello\n".to_vec()
            });
            let subject =
                format!("{prefix}.session.{session_id}.client.ext.terminal.write_stdin");
            let reply =
                nats_request(&nats, subject, serde_json::to_vec(&payload).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "write_stdin error: {resp}");

            // Give `cat` time to echo the data.
            tokio::time::sleep(Duration::from_millis(200)).await;

            // Read the output — it must contain the echoed text.
            let out_req = serde_json::json!({ "sessionId": session_id, "terminalId": tid });
            let subject = format!("{prefix}.session.{session_id}.client.terminal.output");
            let reply =
                nats_request(&nats, subject, serde_json::to_vec(&out_req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "terminal.output error: {resp}");
            let output = resp["output"].as_str().expect("output field");
            assert!(
                output.contains("hello"),
                "expected 'hello' in echoed output, got: {output:?}"
            );

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// `terminal.close_stdin` happy path: close the stdin pipe of a live `cat` process
/// via the dispatcher — `cat` reads EOF and exits cleanly with code 0.
#[tokio::test]
async fn dispatcher_ext_close_stdin_happy_path() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping dispatcher_ext_close_stdin_happy_path");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("close-stdin-ok");
    let session_id = "sess-close-ok";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Spawn `cat`.
            let req = CreateTerminalRequest::new(session_id, "/bin/cat");
            let subject = format!("{prefix}.session.{session_id}.client.terminal.create");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "create error: {resp}");
            let tid = resp["terminalId"].as_str().expect("terminalId").to_string();

            // Write some data first.
            let write_payload = serde_json::json!({
                "terminal_id": tid,
                "data": b"world\n".to_vec()
            });
            let subject =
                format!("{prefix}.session.{session_id}.client.ext.terminal.write_stdin");
            let reply =
                nats_request(&nats, subject, serde_json::to_vec(&write_payload).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "write_stdin error: {resp}");

            // Close stdin — sends EOF; `cat` will exit.
            let close_payload = serde_json::json!({ "terminal_id": tid });
            let subject =
                format!("{prefix}.session.{session_id}.client.ext.terminal.close_stdin");
            let reply =
                nats_request(&nats, subject, serde_json::to_vec(&close_payload).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "close_stdin error: {resp}");

            // Wait for `cat` to exit after EOF.
            let wait_req = serde_json::json!({ "sessionId": session_id, "terminalId": tid });
            let subject =
                format!("{prefix}.session.{session_id}.client.terminal.wait_for_exit");
            let reply =
                nats_request(&nats, subject, serde_json::to_vec(&wait_req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "wait_for_exit error: {resp}");
            let exit_code = resp["exitCode"].as_u64();
            assert_eq!(exit_code, Some(0), "expected cat to exit 0, got: {resp}");

            // Output must contain the echoed data.
            let out_req = serde_json::json!({ "sessionId": session_id, "terminalId": tid });
            let subject = format!("{prefix}.session.{session_id}.client.terminal.output");
            let reply =
                nats_request(&nats, subject, serde_json::to_vec(&out_req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            let output = resp["output"].as_str().expect("output field");
            assert!(
                output.contains("world"),
                "expected 'world' in output after close_stdin, got: {output:?}"
            );

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// `session.request_permission` dispatcher route with `auto_allow_permissions: false`:
/// when no auto-allow is configured, the runtime returns `outcome: "cancelled"`.
#[tokio::test]
async fn dispatcher_session_request_permission_auto_deny_returns_cancelled() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping dispatcher_session_request_permission_auto_deny_returns_cancelled");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("perm-deny");
    let session_id = "sess-perm-deny";

    let nats = async_nats::connect(&nats_url).await.expect("connect");

    // Disable auto-allow — dispatcher must return cancelled.
    let cfg = Config {
        auto_allow_permissions: false,
        ..dispatcher_config(tmp.path().to_path_buf())
    };
    let runtime = Rc::new(WasmRuntime::with_nats(&cfg, Some(nats.clone())).unwrap());
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            let req = serde_json::json!({
                "sessionId": session_id,
                "toolCall": { "toolCallId": "call-deny-1" },
                "options": [
                    { "optionId": "allow", "name": "Allow", "kind": "allow_once" }
                ]
            });
            let subject =
                format!("{prefix}.session.{session_id}.client.session.request_permission");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "request_permission error: {resp}");

            // auto_allow_permissions: false → cancelled outcome.
            let outcome = &resp["outcome"];
            assert_eq!(
                outcome["outcome"].as_str(),
                Some("cancelled"),
                "expected cancelled outcome, got {resp}"
            );

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// `wasm_only: true` causes the dispatcher to reject native terminal creation.
#[tokio::test]
async fn dispatcher_wasm_only_rejects_native_terminal() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!(
                "NATS_TEST_URL not set — skipping dispatcher_wasm_only_rejects_native_terminal"
            );
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("wasm-only");
    let session_id = "sess-wasm-only";

    let nats = async_nats::connect(&nats_url).await.expect("connect");

    let cfg = Config {
        wasm_only: true,
        ..dispatcher_config(tmp.path().to_path_buf())
    };
    let runtime = Rc::new(WasmRuntime::with_nats(&cfg, Some(nats.clone())).unwrap());
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Try to spawn a native process — must be rejected.
            let req = CreateTerminalRequest::new(session_id, "/bin/echo")
                .args(vec!["hello".to_string()]);
            let subject = format!("{prefix}.session.{session_id}.client.terminal.create");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(
                resp.get("error").is_some(),
                "expected error when spawning native in wasm_only mode, got: {resp}"
            );
            let msg = resp["error"]["message"].as_str().unwrap_or("");
            assert!(
                msg.contains("wasm_only") || msg.contains("wasm"),
                "error message should mention wasm_only, got: {msg}"
            );

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// Native terminal that exits with a non-zero code: `wait_for_exit` reports the
/// correct exit code in the response.
#[tokio::test]
async fn dispatcher_native_terminal_nonzero_exit_code() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!(
                "NATS_TEST_URL not set — skipping dispatcher_native_terminal_nonzero_exit_code"
            );
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("nonzero-exit");
    let session_id = "sess-nonzero";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // `/bin/false` exits with code 1.
            let req = CreateTerminalRequest::new(session_id, "/bin/false");
            let subject = format!("{prefix}.session.{session_id}.client.terminal.create");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "create error: {resp}");
            let tid = resp["terminalId"].as_str().expect("terminalId").to_string();

            // Wait for exit.
            let wait_req = serde_json::json!({ "sessionId": session_id, "terminalId": tid });
            let subject =
                format!("{prefix}.session.{session_id}.client.terminal.wait_for_exit");
            let reply =
                nats_request(&nats, subject, serde_json::to_vec(&wait_req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "wait_for_exit error: {resp}");
            let exit_code = resp["exitCode"].as_u64();
            assert_eq!(exit_code, Some(1), "expected exit code 1 from /bin/false, got: {resp}");

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// `wait_for_exit_timeout_secs` kills a hung native process after the configured
/// timeout and returns a signal exit status via the dispatcher.
#[tokio::test]
async fn dispatcher_wait_for_exit_timeout_kills_hung_process() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping dispatcher_wait_for_exit_timeout_kills_hung_process");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("wait-timeout");
    let session_id = "sess-wait-timeout";

    let nats = async_nats::connect(&nats_url).await.expect("connect");

    // 1-second wait timeout so the test finishes quickly.
    let cfg = Config {
        wait_for_exit_timeout_secs: 1,
        ..dispatcher_config(tmp.path().to_path_buf())
    };
    let runtime = Rc::new(WasmRuntime::with_nats(&cfg, Some(nats.clone())).unwrap());
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Spawn a process that will never exit on its own.
            let req = CreateTerminalRequest::new(session_id, "/bin/sleep")
                .args(vec!["60".to_string()]);
            let subject = format!("{prefix}.session.{session_id}.client.terminal.create");
            let reply = nats_request(&nats, subject, serde_json::to_vec(&req).unwrap()).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(resp.get("error").is_none(), "create error: {resp}");
            let tid = resp["terminalId"].as_str().expect("terminalId").to_string();

            // Wait for exit — the runtime will kill after 1s and return a signal.
            let wait_req = serde_json::json!({ "sessionId": session_id, "terminalId": tid });
            let subject =
                format!("{prefix}.session.{session_id}.client.terminal.wait_for_exit");
            // Use a generous NATS timeout (10s) — the process kill takes ~1s.
            let msg = tokio::time::timeout(
                Duration::from_secs(10),
                nats.request(subject, serde_json::to_vec(&wait_req).unwrap().into()),
            )
            .await
            .expect("request timed out")
            .expect("NATS error");
            let resp: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
            assert!(resp.get("error").is_none(), "wait_for_exit error: {resp}");

            // The process was killed — response must carry a signal (not an exit code).
            assert!(
                resp.get("signal").is_some(),
                "expected signal in response after timeout kill, got: {resp}"
            );

            let _ = shutdown_tx.send(true);
        })
        .await;
}

/// A NATS message whose subject matches the dispatcher subscription pattern
/// (`{prefix}.session.*.client.>`) but fails `parse_client_subject` is silently
/// ignored — the dispatcher logs a warning and continues processing.
///
/// Exercises the `None` branch of `parse_client_subject` in dispatcher.rs.
#[tokio::test]
async fn dispatcher_malformed_subject_ignored() {
    let nats_url = match nats_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping dispatcher_malformed_subject_ignored");
            return;
        }
    };
    let tmp = TempDir::new().unwrap();
    let prefix = test_prefix("malformed");
    let session_id = "sess-malformed";

    let nats = async_nats::connect(&nats_url).await.expect("connect");
    let runtime = Rc::new(
        WasmRuntime::with_nats(&dispatcher_config(tmp.path().to_path_buf()), Some(nats.clone()))
            .unwrap(),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::task::spawn_local(trogon_wasm_runtime::dispatcher::run(
                nats.clone(),
                prefix.clone(),
                Rc::clone(&runtime),
                shutdown_rx,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Publish to a subject that matches the subscription wildcard but has
            // no recognisable method suffix — parse_client_subject returns None.
            // "unknown" is not a valid ClientMethod suffix per acp-nats parsing.
            let bad_subject = format!("{prefix}.session.{session_id}.client.unknown");
            nats_publish(&nats, bad_subject, bytes::Bytes::from_static(b"{}")).await;

            tokio::time::sleep(Duration::from_millis(200)).await;

            // Dispatcher must still be alive after ignoring the malformed subject.
            let subject =
                format!("{prefix}.session.{session_id}.client.ext.runtime.list_sessions");
            let reply = nats_request(&nats, subject, bytes::Bytes::from_static(b"{}")).await;
            let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            assert!(
                resp.get("sessions").is_some(),
                "dispatcher should be alive after malformed subject: {resp}"
            );

            let _ = shutdown_tx.send(true);
        })
        .await;
}
