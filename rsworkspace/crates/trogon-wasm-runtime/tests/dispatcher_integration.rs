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
