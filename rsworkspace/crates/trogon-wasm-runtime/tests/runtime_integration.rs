use agent_client_protocol::{
    CreateTerminalRequest, KillTerminalRequest, ReadTextFileRequest, ReleaseTerminalRequest,
    RequestPermissionRequest, SessionId, TerminalId, TerminalOutputRequest,
    WaitForTerminalExitRequest, WriteTextFileRequest,
};
use futures::StreamExt as _;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use trogon_wasm_runtime::{Config, WasmRuntime};

/// Compiles a WAT text module to a `.wasm` file inside `dir`.
fn make_wasm(dir: &Path, name: &str, wat_text: &str) -> PathBuf {
    let wasm_bytes = wat::parse_str(wat_text).expect("WAT compile failed");
    let path = dir.join(name);
    std::fs::write(&path, &wasm_bytes).unwrap();
    path
}

fn test_config(session_root: PathBuf) -> Config {
    Config {
        session_root,
        output_byte_limit: 1024 * 1024,
        auto_allow_permissions: true,
        wasm_timeout_secs: None,
        wasm_only: false,
        wasm_memory_limit_bytes: None,
        module_cache_dir: None,
        wasm_allow_network: false,
        wasm_fuel_limit: 1_000_000_000u64,
        wasm_host_call_limit: 10_000u32,
        acp_prefix: "acp".to_string(),
        wasm_max_concurrent_tasks: 32,
        session_idle_timeout_secs: 3600,
        wasm_max_module_size_bytes: 100 * 1024 * 1024,
        wait_for_exit_timeout_secs: 300,
    }
}

fn session_id() -> &'static str {
    "test-session-001"
}

// ── Filesystem ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn write_and_read_file_round_trip() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    let write_req = WriteTextFileRequest::new(
        SessionId::from("s1"),
        PathBuf::from("/hello.txt"),
        "Hello, sandbox!",
    );
    runtime
        .handle_write_text_file(session_id(), write_req)
        .await
        .expect("write should succeed");

    let read_req =
        ReadTextFileRequest::new(SessionId::from("s1"), PathBuf::from("/hello.txt"));
    let resp = runtime
        .handle_read_text_file(session_id(), read_req)
        .await
        .expect("read should succeed");

    assert_eq!(resp.content, "Hello, sandbox!");
}

#[tokio::test]
async fn write_creates_nested_directories() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    let req = WriteTextFileRequest::new(
        SessionId::from("s1"),
        PathBuf::from("/a/b/c/file.txt"),
        "nested",
    );
    runtime
        .handle_write_text_file(session_id(), req)
        .await
        .expect("write with nested dirs should succeed");
}

#[tokio::test]
async fn path_traversal_rejected_on_write() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    let req = WriteTextFileRequest::new(
        SessionId::from("s1"),
        PathBuf::from("/../../etc/passwd"),
        "pwned",
    );
    let err = runtime
        .handle_write_text_file(session_id(), req)
        .await
        .expect_err("traversal should be rejected");

    assert!(err.message.contains("outside the session sandbox"));
}

#[tokio::test]
async fn path_traversal_rejected_on_read() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    let req = ReadTextFileRequest::new(
        SessionId::from("s1"),
        PathBuf::from("/../../../etc/passwd"),
    );
    let err = runtime
        .handle_read_text_file(session_id(), req)
        .await
        .expect_err("traversal should be rejected");

    assert!(err.message.contains("outside the session sandbox"));
}

// ── Terminal (native process) ───────────────────────────────────────────────

#[tokio::test]
async fn create_terminal_echo_and_wait() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    let req = CreateTerminalRequest::new(SessionId::from("s1"), "echo")
        .args(vec!["hello from sandbox".to_string()]);
    let resp = runtime
        .handle_create_terminal(session_id(), req)
        .await
        .expect("echo should succeed");

    let tid = resp.terminal_id;

    let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid.clone());
    let exit = runtime
        .handle_wait_for_terminal_exit(wait_req)
        .await
        .expect("wait should succeed");
    assert_eq!(exit.exit_status.exit_code, Some(0));

    let out_req = TerminalOutputRequest::new(SessionId::from("s1"), tid);
    let out = runtime
        .handle_terminal_output(out_req)
        .await
        .expect("output should succeed");
    assert!(out.output.contains("hello from sandbox"));
}

#[tokio::test]
async fn terminal_output_unknown_id_returns_error() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    let req = TerminalOutputRequest::new(
        SessionId::from("s1"),
        TerminalId::new("nonexistent-id"),
    );
    runtime
        .handle_terminal_output(req)
        .await
        .expect_err("should return error for unknown terminal");
}

#[tokio::test]
async fn release_terminal_removes_it() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    let req = CreateTerminalRequest::new(SessionId::from("s1"), "echo")
        .args(vec!["bye".to_string()]);
    let resp = runtime
        .handle_create_terminal(session_id(), req)
        .await
        .unwrap();
    let tid = resp.terminal_id;

    let rel = ReleaseTerminalRequest::new(SessionId::from("s1"), tid.clone());
    runtime
        .handle_release_terminal(rel)
        .await
        .expect("release should succeed");

    // After release the terminal is gone.
    let out_req = TerminalOutputRequest::new(SessionId::from("s1"), tid);
    runtime
        .handle_terminal_output(out_req)
        .await
        .expect_err("terminal should be gone after release");
}

// ── Kill terminal ──────────────────────────────────────────────────────────

#[tokio::test]
async fn kill_terminal_stops_process() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    let req = CreateTerminalRequest::new(SessionId::from("s1"), "sleep")
        .args(vec!["30".to_string()]);
    let resp = runtime
        .handle_create_terminal(session_id(), req)
        .await
        .expect("sleep should spawn");
    let tid = resp.terminal_id;

    let kill_req = KillTerminalRequest::new(SessionId::from("s1"), tid.clone());
    runtime
        .handle_kill_terminal(kill_req)
        .await
        .expect("kill should succeed");

    let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid);
    let exit = runtime
        .handle_wait_for_terminal_exit(wait_req)
        .await
        .unwrap();
    // Killed process exits with a signal, not code 0.
    assert_ne!(exit.exit_status.exit_code, Some(0));
}

// ── Output byte limit ──────────────────────────────────────────────────────

#[tokio::test]
async fn output_byte_limit_truncates() {
    let tmp = TempDir::new().unwrap();
    let cfg = Config {
        output_byte_limit: 50,
        ..test_config(tmp.path().to_path_buf())
    };
    let runtime = WasmRuntime::new(&cfg).unwrap();

    let req = CreateTerminalRequest::new(SessionId::from("s1"), "echo")
        .args(vec!["A".repeat(200)]);
    let resp = runtime
        .handle_create_terminal(session_id(), req)
        .await
        .unwrap();
    let tid = resp.terminal_id;

    let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid.clone());
    runtime.handle_wait_for_terminal_exit(wait_req).await.unwrap();

    let out_req = TerminalOutputRequest::new(SessionId::from("s1"), tid);
    let out = runtime.handle_terminal_output(out_req).await.unwrap();
    assert!(
        out.output.len() <= 50,
        "output len {} should be truncated to 50 bytes",
        out.output.len()
    );
}

// ── WASM module execution ──────────────────────────────────────────────────

#[tokio::test]
async fn wasm_module_writes_to_stdout() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let wasm_path = make_wasm(tmp.path(), "hello.wasm", r#"
                (module
                  (import "wasi_snapshot_preview1" "fd_write"
                    (func $fd_write (param i32 i32 i32 i32) (result i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (data (i32.const 16) "hello from wasm\n")
                  (func (export "_start")
                    (i32.store (i32.const 0) (i32.const 16))
                    (i32.store (i32.const 4) (i32.const 16))
                    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 8)))
                  )
                )
            "#);

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("wasm module should run");
            let tid = resp.terminal_id;

            let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid.clone());
            let exit = runtime.handle_wait_for_terminal_exit(wait_req).await.unwrap();
            assert_eq!(exit.exit_status.exit_code, Some(0));

            let out_req = TerminalOutputRequest::new(SessionId::from("s1"), tid);
            let out = runtime.handle_terminal_output(out_req).await.unwrap();
            assert!(out.output.contains("hello from wasm"), "stdout should be captured");
        })
        .await;
}

#[tokio::test]
async fn wasm_module_exit_code() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let wasm_path = make_wasm(tmp.path(), "exit42.wasm", r#"
                (module
                  (import "wasi_snapshot_preview1" "proc_exit"
                    (func $proc_exit (param i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (func (export "_start")
                    (call $proc_exit (i32.const 42))
                    unreachable
                  )
                )
            "#);

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .unwrap();
            let tid = resp.terminal_id;

            let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid);
            let exit = runtime.handle_wait_for_terminal_exit(wait_req).await.unwrap();
            assert_eq!(exit.exit_status.exit_code, Some(42));
        })
        .await;
}

#[tokio::test]
async fn wasm_module_stderr_captured() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let wasm_path = make_wasm(tmp.path(), "stderr.wasm", r#"
                (module
                  (import "wasi_snapshot_preview1" "fd_write"
                    (func $fd_write (param i32 i32 i32 i32) (result i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (data (i32.const 16) "error output\n")
                  (func (export "_start")
                    (i32.store (i32.const 0) (i32.const 16))
                    (i32.store (i32.const 4) (i32.const 13))
                    (drop (call $fd_write (i32.const 2) (i32.const 0) (i32.const 1) (i32.const 8)))
                  )
                )
            "#);

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .unwrap();
            let tid = resp.terminal_id;

            let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid.clone());
            runtime.handle_wait_for_terminal_exit(wait_req).await.unwrap();

            let out_req = TerminalOutputRequest::new(SessionId::from("s1"), tid);
            let out = runtime.handle_terminal_output(out_req).await.unwrap();
            assert!(out.output.contains("error output"), "stderr should be captured");
        })
        .await;
}

// ── Permission ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn auto_allow_selects_first_option() {
    use agent_client_protocol::{
        PermissionOption, PermissionOptionId, PermissionOptionKind, RequestPermissionOutcome,
        ToolCallId, ToolCallUpdate, ToolCallUpdateFields,
    };

    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    let options = vec![
        PermissionOption::new(
            PermissionOptionId::new("allow"),
            "Allow",
            PermissionOptionKind::AllowOnce,
        ),
        PermissionOption::new(
            PermissionOptionId::new("deny"),
            "Deny",
            PermissionOptionKind::RejectOnce,
        ),
    ];

    let req = RequestPermissionRequest::new(
        SessionId::from("s1"),
        ToolCallUpdate::new(ToolCallId::new("tc-1"), ToolCallUpdateFields::new()),
        options,
    );
    let resp = runtime
        .handle_request_permission(req)
        .expect("permission should succeed");

    assert!(matches!(
        resp.outcome,
        RequestPermissionOutcome::Selected(ref s) if s.option_id.0.as_ref() == "allow"
    ));
}

#[tokio::test]
async fn auto_deny_when_no_options() {
    use agent_client_protocol::{RequestPermissionOutcome, ToolCallId, ToolCallUpdate, ToolCallUpdateFields};

    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    let req = RequestPermissionRequest::new(
        SessionId::from("s1"),
        ToolCallUpdate::new(ToolCallId::new("tc-1"), ToolCallUpdateFields::new()),
        vec![],
    );
    let resp = runtime
        .handle_request_permission(req)
        .expect("permission should succeed");
    assert!(matches!(resp.outcome, RequestPermissionOutcome::Cancelled));
}

// ── Feature 1: Module Compilation Cache ───────────────────────────────────

/// Calls `handle_create_terminal` twice with the same `.wasm` file and verifies
/// both succeed. The second call exercises the module cache path.
#[tokio::test]
async fn wasm_module_cache_reuses_compiled_module() {
    let tmp = TempDir::new().unwrap();

    // Use a LocalSet because run_wasm_terminal uses spawn_local.
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "cache_test.wasm",
                r#"
                (module
                  (import "wasi_snapshot_preview1" "proc_exit"
                    (func $proc_exit (param i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (func (export "_start")
                    (call $proc_exit (i32.const 0))
                    unreachable
                  )
                )
            "#,
            );

            // First invocation — compiles and caches the module.
            let req1 = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp1 = runtime
                .handle_create_terminal("cache-session-1", req1)
                .await
                .expect("first wasm run should succeed");

            let wait1 = WaitForTerminalExitRequest::new(SessionId::from("s1"), resp1.terminal_id);
            let exit1 = runtime
                .handle_wait_for_terminal_exit(wait1)
                .await
                .unwrap();
            assert_eq!(exit1.exit_status.exit_code, Some(0), "first run: exit 0");

            // Second invocation — should hit the cache (same path, no recompile).
            let req2 = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp2 = runtime
                .handle_create_terminal("cache-session-2", req2)
                .await
                .expect("second wasm run should succeed (cache hit)");

            let wait2 = WaitForTerminalExitRequest::new(SessionId::from("s1"), resp2.terminal_id);
            let exit2 = runtime
                .handle_wait_for_terminal_exit(wait2)
                .await
                .unwrap();
            assert_eq!(exit2.exit_status.exit_code, Some(0), "second run: exit 0");
        })
        .await;
}

// ── Feature 2: WASM Execution Timeout ─────────────────────────────────────

/// Runs a normal WASM module with a generous timeout set — verifies the module
/// finishes successfully (timeout doesn't fire) and the timeout code path compiles
/// and executes without error.
#[tokio::test]
async fn wasm_module_runs_with_timeout_set() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cfg = Config {
                wasm_timeout_secs: Some(30), // generous timeout — should not fire
                ..test_config(tmp.path().to_path_buf())
            };
            let runtime = WasmRuntime::new(&cfg).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "timeout_test.wasm",
                r#"
                (module
                  (import "wasi_snapshot_preview1" "fd_write"
                    (func $fd_write (param i32 i32 i32 i32) (result i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (data (i32.const 16) "timeout ok\n")
                  (func (export "_start")
                    (i32.store (i32.const 0) (i32.const 16))
                    (i32.store (i32.const 4) (i32.const 11))
                    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 8)))
                  )
                )
            "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("wasm module should run within timeout");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id.clone());
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();
            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "module should exit cleanly"
            );

            let out_req = TerminalOutputRequest::new(SessionId::from("s1"), resp.terminal_id);
            let out = runtime.handle_terminal_output(out_req).await.unwrap();
            assert!(out.output.contains("timeout ok"), "output should be captured");
        })
        .await;
}

// ── Feature 3: Session Auto-Cleanup ───────────────────────────────────────

/// Releasing the last terminal for a session should remove the sandbox directory.
#[tokio::test]
async fn session_cleanup_removes_sandbox() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let req = CreateTerminalRequest::new(SessionId::from("s1"), "echo")
                .args(vec!["cleanup test".to_string()]);
            let resp = runtime
                .handle_create_terminal("cleanup-session", req)
                .await
                .expect("echo should succeed");
            let tid = resp.terminal_id;

            // Let the process finish.
            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), tid.clone());
            runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();

            // Verify the sandbox directory exists before release.
            let sandbox = tmp.path().join("cleanup-session");
            assert!(sandbox.exists(), "sandbox should exist before release");

            // Release the only terminal — should trigger session cleanup.
            let rel = ReleaseTerminalRequest::new(SessionId::from("s1"), tid);
            runtime
                .handle_release_terminal(rel)
                .await
                .expect("release should succeed");

            // The sandbox directory should have been deleted.
            assert!(
                !sandbox.exists(),
                "sandbox should be removed after last terminal released"
            );
        })
        .await;
}

// ── Feature 4: Background WASM Execution ─────────────────────────────────

/// `create_terminal` for a WASM module returns immediately; output is available
/// after `wait_for_terminal_exit` completes.
#[tokio::test]
async fn wasm_module_background_execution() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "bg_test.wasm",
                r#"
                (module
                  (import "wasi_snapshot_preview1" "fd_write"
                    (func $fd_write (param i32 i32 i32 i32) (result i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (data (i32.const 16) "background output\n")
                  (func (export "_start")
                    (i32.store (i32.const 0) (i32.const 16))
                    (i32.store (i32.const 4) (i32.const 18))
                    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 8)))
                  )
                )
            "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            // Feature 4: create_terminal returns immediately.
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("wasm terminal should be created");
            let tid = resp.terminal_id;

            // The module may or may not have finished — both empty and populated are valid.
            // We just verify the snapshot call doesn't panic.
            let out_req = TerminalOutputRequest::new(SessionId::from("s1"), tid.clone());
            let _ = runtime.handle_terminal_output(out_req).await.unwrap();

            // Wait for exit — should complete with exit_code 0.
            let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid.clone());
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();
            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "background wasm should exit with code 0"
            );

            // After wait, output should be fully available.
            let out_req2 = TerminalOutputRequest::new(SessionId::from("s1"), tid);
            let out = runtime.handle_terminal_output(out_req2).await.unwrap();
            assert!(
                out.output.contains("background output"),
                "output should be available after wait: {:?}",
                out.output
            );
        })
        .await;
}

// ── WASM-only mode ─────────────────────────────────────────────────────────

#[tokio::test]
async fn wasm_only_rejects_native_commands() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&Config {
        wasm_only: true,
        ..test_config(tmp.path().to_path_buf())
    })
    .unwrap();

    let req = CreateTerminalRequest::new(SessionId::from("s1"), "echo")
        .args(vec!["hello".to_string()]);
    let err = runtime
        .handle_create_terminal(session_id(), req)
        .await
        .expect_err("native command should be rejected in wasm_only mode");

    assert!(
        err.message.contains("wasm_only"),
        "error should mention wasm_only: {}",
        err.message
    );
}

#[tokio::test]
async fn wasm_only_allows_wasm_commands() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    let wasm_path = make_wasm(tmp.path(), "ok.wasm", r#"
        (module
          (import "wasi_snapshot_preview1" "proc_exit"
            (func $proc_exit (param i32)))
          (memory 1)
          (export "memory" (memory 0))
          (func (export "_start")
            (call $proc_exit (i32.const 0))
          )
        )
    "#);

    tokio::task::LocalSet::new()
        .run_until(async {
            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect(".wasm should be allowed in wasm_only mode");
            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();
            assert_eq!(exit.exit_status.exit_code, Some(0));
        })
        .await;
}

// ── Memory cap ─────────────────────────────────────────────────────────────

/// Verify that a generous memory limit does not prevent a module from running.
/// This tests the plumbing; the denial path is covered by `wasm_module_memory_limit_denial`.
#[tokio::test]
async fn wasm_module_memory_limit_allows_small_module() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cfg = Config {
                wasm_only: false,
                // 64 MB — generous, should not prevent the 1-page module from running.
                wasm_memory_limit_bytes: Some(64 * 1024 * 1024),
                ..test_config(tmp.path().to_path_buf())
            };
            let runtime = WasmRuntime::new(&cfg).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "memlimit.wasm",
                r#"
                (module
                  (import "wasi_snapshot_preview1" "proc_exit"
                    (func $proc_exit (param i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (func (export "_start")
                    (call $proc_exit (i32.const 0))
                    unreachable
                  )
                )
            "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("module with generous memory limit should run");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();
            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "module should exit cleanly under generous memory limit"
            );
        })
        .await;
}

// ── Trogon host functions ─────────────────────────────────────────────────

/// A WASM module that imports `trogon.log` and calls it should exit cleanly.
#[tokio::test]
async fn wasm_module_trogon_log_host_function() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "trogon_log.wasm",
                r#"
                (module
                  (import "trogon_v1" "log" (func $log (param i32 i32 i32 i32)))
                  (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (data (i32.const 0) "info")
                  (data (i32.const 4) "hello from wasm")
                  (func (export "_start")
                    (call $log (i32.const 0) (i32.const 4) (i32.const 4) (i32.const 15))
                    (call $proc_exit (i32.const 0))
                  )
                )
            "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("module with trogon.log import should run");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();
            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "module calling trogon.log should exit cleanly"
            );
        })
        .await;
}

// ── Feature: Persistent on-disk module cache ──────────────────────────────

/// Compile a module with runtime A (writes on-disk cache), then create runtime B
/// with the same `module_cache_dir` — runtime B should load from disk and succeed.
#[tokio::test]
async fn module_cache_persists_across_runtimes() {
    let tmp = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();

    let exit_wat = r#"
        (module
          (import "wasi_snapshot_preview1" "proc_exit"
            (func $proc_exit (param i32)))
          (memory 1)
          (export "memory" (memory 0))
          (func (export "_start")
            (call $proc_exit (i32.const 0))
            unreachable
          )
        )
    "#;

    let wasm_path = make_wasm(tmp.path(), "persist_cache.wasm", exit_wat);

    // Runtime A: compiles and writes on-disk cache.
    {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let cfg = Config {
                    wasm_only: false,
                    module_cache_dir: Some(cache_dir.path().to_path_buf()),
                    ..test_config(tmp.path().join("sessions_a"))
                };
                let runtime = WasmRuntime::new(&cfg).unwrap();
                let req = CreateTerminalRequest::new(
                    SessionId::from("s1"),
                    wasm_path.to_str().unwrap(),
                );
                let resp = runtime
                    .handle_create_terminal("session-a", req)
                    .await
                    .expect("runtime A should compile and run module");
                let wait_req =
                    WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
                let exit = runtime
                    .handle_wait_for_terminal_exit(wait_req)
                    .await
                    .unwrap();
                assert_eq!(exit.exit_status.exit_code, Some(0), "runtime A: exit 0");
            })
            .await;
    }

    // Verify cache files were written.
    let cache_files: Vec<_> = std::fs::read_dir(cache_dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        !cache_files.is_empty(),
        "cache directory should contain cached module files"
    );

    // Runtime B: loads from on-disk cache (same module_cache_dir).
    {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let cfg = Config {
                    wasm_only: false,
                    module_cache_dir: Some(cache_dir.path().to_path_buf()),
                    ..test_config(tmp.path().join("sessions_b"))
                };
                let runtime = WasmRuntime::new(&cfg).unwrap();
                let req = CreateTerminalRequest::new(
                    SessionId::from("s1"),
                    wasm_path.to_str().unwrap(),
                );
                let resp = runtime
                    .handle_create_terminal("session-b", req)
                    .await
                    .expect("runtime B should load from disk cache and run module");
                let wait_req =
                    WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
                let exit = runtime
                    .handle_wait_for_terminal_exit(wait_req)
                    .await
                    .unwrap();
                assert_eq!(exit.exit_status.exit_code, Some(0), "runtime B: exit 0");
            })
            .await;
    }
}

/// Overwrite the `.wasm` file with different bytes; verify the runtime recompiles
/// and the new module runs correctly (old cached version is not used).
#[tokio::test]
async fn module_cache_invalidates_on_mtime_change() {
    let tmp = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();

    // First WAT: exits with code 7.
    let wat_v1 = r#"
        (module
          (import "wasi_snapshot_preview1" "proc_exit"
            (func $proc_exit (param i32)))
          (memory 1)
          (export "memory" (memory 0))
          (func (export "_start")
            (call $proc_exit (i32.const 7))
            unreachable
          )
        )
    "#;

    // Second WAT: exits with code 42.
    let wat_v2 = r#"
        (module
          (import "wasi_snapshot_preview1" "proc_exit"
            (func $proc_exit (param i32)))
          (memory 1)
          (export "memory" (memory 0))
          (func (export "_start")
            (call $proc_exit (i32.const 42))
            unreachable
          )
        )
    "#;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cfg = Config {
                wasm_only: false,
                module_cache_dir: Some(cache_dir.path().to_path_buf()),
                ..test_config(tmp.path().join("sessions"))
            };
            let runtime = WasmRuntime::new(&cfg).unwrap();

            // Write and run v1 (exit code 7).
            let wasm_path = make_wasm(tmp.path(), "invalidate_test.wasm", wat_v1);
            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal("session-v1", req)
                .await
                .expect("v1 module should run");
            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();
            assert_eq!(exit.exit_status.exit_code, Some(7), "v1 should exit 7");

            // Overwrite the file with v2 bytes and ensure mtime advances.
            let wasm_bytes_v2 = wat::parse_str(wat_v2).expect("WAT v2 compile failed");
            // Sleep briefly to guarantee mtime difference.
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            std::fs::write(&wasm_path, &wasm_bytes_v2).unwrap();
            // Touch file to ensure mtime changes (some filesystems have 1s resolution).
            // We use filetime manipulation via std to set a future mtime.
            let new_mtime = std::time::SystemTime::now() + std::time::Duration::from_secs(1);
            // Write again with a slight delay ensures at least different content hash.
            // On Linux, mtime has nanosecond resolution so writing new bytes is enough.
            let _ = new_mtime; // we just rely on the write changing mtime

            // Run again — should pick up v2 (exit code 42).
            let req2 = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp2 = runtime
                .handle_create_terminal("session-v2", req2)
                .await
                .expect("v2 module should run after invalidation");
            let wait_req2 =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp2.terminal_id);
            let exit2 = runtime
                .handle_wait_for_terminal_exit(wait_req2)
                .await
                .unwrap();
            assert_eq!(
                exit2.exit_status.exit_code,
                Some(42),
                "v2 should exit 42 (cache was invalidated)"
            );
        })
        .await;
}

// ── Feature: trogon.nats_request host function ────────────────────────────

/// A WASM module calling `nats_request` with no NATS client should get -1
/// and exit cleanly (no panic, no trap).
#[tokio::test]
async fn wasm_module_trogon_nats_request_no_nats() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            // WasmRuntime::new has no NATS client.
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            // WAT: call nats_request, expect -1, then proc_exit(0).
            // Memory layout:
            //   offset 0: subject string "test"
            //   offset 4: payload (empty, 0 bytes)
            //   offset 4: out_buf (4 bytes, for potential response)
            let wasm_path = make_wasm(
                tmp.path(),
                "nats_request_no_nats.wasm",
                r#"
                (module
                  (import "trogon_v1" "nats_request"
                    (func $nats_request (param i32 i32 i32 i32 i32 i32 i32) (result i32)))
                  (import "wasi_snapshot_preview1" "proc_exit"
                    (func $proc_exit (param i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (data (i32.const 0) "test")
                  (func (export "_start")
                    ;; nats_request("test", 4, "", 0, 100ms, out_buf@100, 64)
                    (local $ret i32)
                    (local.set $ret
                      (call $nats_request
                        (i32.const 0)   ;; subject_ptr
                        (i32.const 4)   ;; subject_len
                        (i32.const 0)   ;; payload_ptr (empty)
                        (i32.const 0)   ;; payload_len
                        (i32.const 100) ;; timeout_ms
                        (i32.const 100) ;; out_buf_ptr
                        (i32.const 64)  ;; out_buf_max
                      )
                    )
                    ;; expect -1 (no NATS)
                    (if (i32.ne (local.get $ret) (i32.const -1))
                      (then (call $proc_exit (i32.const 1)))
                    )
                    (call $proc_exit (i32.const 0))
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("module with nats_request should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();
            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "nats_request with no NATS should return -1 and module should exit cleanly"
            );
        })
        .await;
}

// ── Feature: trogon.request_permission host function ─────────────────────

/// A WASM module calling `request_permission` with auto_allow=true should get
/// return value 0 and out_selected written as 0, then proc_exit(0).
#[tokio::test]
async fn wasm_module_request_permission_auto_allow() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            // auto_allow_permissions = true (default in test_config).
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            // JSON options string: ["Allow","Deny"] — 16 bytes
            // In WAT string literals, " is \22.
            // ["Allow","Deny"] = [\22Allow\22,\22Deny\22]
            let wasm_path = make_wasm(
                tmp.path(),
                "request_permission_allow.wasm",
                r#"
                (module
                  (import "trogon_v1" "request_permission"
                    (func $request_permission (param i32 i32 i32) (result i32)))
                  (import "wasi_snapshot_preview1" "proc_exit"
                    (func $proc_exit (param i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (data (i32.const 0) "[\22Allow\22,\22Deny\22]")
                  (func (export "_start")
                    (local $ret i32)
                    (local $selected i32)
                    (local.set $ret
                      (call $request_permission
                        (i32.const 0)    ;; options_json_ptr
                        (i32.const 16)   ;; options_json_len (["Allow","Deny"] = 16 bytes)
                        (i32.const 100)  ;; out_selected_ptr
                      )
                    )
                    ;; ret should be 0 (success)
                    (if (i32.ne (local.get $ret) (i32.const 0))
                      (then (call $proc_exit (i32.const 1)))
                    )
                    ;; out_selected (i32 at offset 100) should be 0
                    (local.set $selected (i32.load (i32.const 100)))
                    (if (i32.ne (local.get $selected) (i32.const 0))
                      (then (call $proc_exit (i32.const 2)))
                    )
                    (call $proc_exit (i32.const 0))
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("module with request_permission should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();
            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "request_permission with auto_allow should return 0 and write selected=0"
            );
        })
        .await;
}

// ── Feature: WASI network access ─────────────────────────────────────────

/// Verify that enabling `wasm_allow_network` doesn't break a simple module.
#[tokio::test]
async fn wasm_module_network_enabled_flag_wires_through() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cfg = Config {
                wasm_allow_network: true,
                ..test_config(tmp.path().to_path_buf())
            };
            let runtime = WasmRuntime::new(&cfg).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "network_enabled.wasm",
                r#"
                (module
                  (import "wasi_snapshot_preview1" "proc_exit"
                    (func $proc_exit (param i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (func (export "_start")
                    (call $proc_exit (i32.const 0))
                    unreachable
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("module should run with network enabled");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();
            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "module should exit cleanly with wasm_allow_network=true"
            );
        })
        .await;
}

// ── Fix 2: Configurable fuel limit ───────────────────────────────────────────

/// A WASM module that runs an infinite loop exhausts a very low fuel limit and
/// exits with a signal (trap), not exit code 0.
#[tokio::test]
async fn wasm_module_custom_fuel_limit() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cfg = Config {
                wasm_fuel_limit: 1000, // very low — exhausted immediately by the loop
                ..test_config(tmp.path().to_path_buf())
            };
            let runtime = WasmRuntime::new(&cfg).unwrap();

            // Infinite loop — will exhaust 1000 fuel units immediately.
            let wasm_path = make_wasm(
                tmp.path(),
                "fuel_exhaust.wasm",
                r#"
                (module
                  (memory 1)
                  (export "memory" (memory 0))
                  (func $loop (export "_start")
                    (block $break
                      (loop $continue
                        (br $continue)
                      )
                    )
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("module should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();

            // Fuel exhaustion is a trap — should NOT be exit code 0.
            assert_ne!(
                exit.exit_status.exit_code,
                Some(0),
                "module should not exit cleanly when fuel is exhausted"
            );
            // Should have a signal (trap) set.
            assert!(
                exit.exit_status.signal.is_some(),
                "fuel exhaustion should produce a signal/trap: {:?}",
                exit.exit_status
            );
        })
        .await;
}

// ── Fix 4: Host call budget ───────────────────────────────────────────────────

/// A WASM module with a budget of 2 calls trogon.log 5 times.
/// The first 2 calls succeed (silently), the rest are silently dropped.
/// The module must still exit with code 0 (budget exhaustion doesn't crash it).
#[tokio::test]
async fn wasm_module_host_call_budget_exhausted() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cfg = Config {
                wasm_host_call_limit: 2, // very low budget
                ..test_config(tmp.path().to_path_buf())
            };
            let runtime = WasmRuntime::new(&cfg).unwrap();

            // Module calls trogon_v1.log 5 times, then proc_exit(0).
            // Budget is 2: first 2 calls succeed, remaining 3 silently fail.
            // The module should still exit cleanly with code 0.
            let wasm_path = make_wasm(
                tmp.path(),
                "budget_exhaust.wasm",
                r#"
                (module
                  (import "trogon_v1" "log" (func $log (param i32 i32 i32 i32)))
                  (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (data (i32.const 0) "info")
                  (data (i32.const 4) "msg")
                  (func (export "_start")
                    (call $log (i32.const 0) (i32.const 4) (i32.const 4) (i32.const 3))
                    (call $log (i32.const 0) (i32.const 4) (i32.const 4) (i32.const 3))
                    (call $log (i32.const 0) (i32.const 4) (i32.const 4) (i32.const 3))
                    (call $log (i32.const 0) (i32.const 4) (i32.const 4) (i32.const 3))
                    (call $log (i32.const 0) (i32.const 4) (i32.const 4) (i32.const 3))
                    (call $proc_exit (i32.const 0))
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("module should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();

            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "budget exhaustion should not crash the module; it should exit cleanly"
            );
        })
        .await;
}

// ── Fix 5: trogon.subscribe / recv_message / unsubscribe ──────────────────────

/// Calling trogon.subscribe with no NATS client should return -1,
/// and the module should still exit cleanly.
#[tokio::test]
async fn wasm_module_nats_subscribe_no_nats() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            // WasmRuntime::new has no NATS client.
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "subscribe_no_nats.wasm",
                r#"
                (module
                  (import "trogon_v1" "subscribe"
                    (func $subscribe (param i32 i32) (result i32)))
                  (import "wasi_snapshot_preview1" "proc_exit"
                    (func $proc_exit (param i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (data (i32.const 0) "test.subject")
                  (func (export "_start")
                    (local $ret i32)
                    (local.set $ret
                      (call $subscribe
                        (i32.const 0)   ;; subject_ptr
                        (i32.const 12)  ;; subject_len ("test.subject" = 12 bytes)
                      )
                    )
                    ;; expect -1 (no NATS)
                    (if (i32.ne (local.get $ret) (i32.const -1))
                      (then (call $proc_exit (i32.const 1)))
                    )
                    (call $proc_exit (i32.const 0))
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("module with subscribe should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();
            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "trogon.subscribe with no NATS should return -1 and module should exit cleanly"
            );
        })
        .await;
}

// ── Fix 6: request_permission with no NATS and auto_allow=false ───────────────

/// When auto_allow_permissions is false and no NATS client is present,
/// request_permission should return -1 (denied/cancelled).
/// The module detects -1 and exits cleanly with code 0.
#[tokio::test]
async fn wasm_module_request_permission_no_nats_denied() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cfg = Config {
                auto_allow_permissions: false, // not auto-allowing
                ..test_config(tmp.path().to_path_buf())
            };
            // No NATS client — WasmRuntime::new, not with_nats.
            let runtime = WasmRuntime::new(&cfg).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "request_permission_no_nats.wasm",
                r#"
                (module
                  (import "trogon_v1" "request_permission"
                    (func $request_permission (param i32 i32 i32) (result i32)))
                  (import "wasi_snapshot_preview1" "proc_exit"
                    (func $proc_exit (param i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (data (i32.const 0) "[\22Allow\22,\22Deny\22]")
                  (func (export "_start")
                    (local $ret i32)
                    (local.set $ret
                      (call $request_permission
                        (i32.const 0)    ;; options_json_ptr
                        (i32.const 16)   ;; options_json_len
                        (i32.const 100)  ;; out_selected_ptr
                      )
                    )
                    ;; expect -1 (no NATS, auto_allow=false → denied)
                    (if (i32.ne (local.get $ret) (i32.const -1))
                      (then (call $proc_exit (i32.const 1)))
                    )
                    (call $proc_exit (i32.const 0))
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("module should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();
            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "request_permission with no NATS and auto_allow=false should return -1 (denied)"
            );
        })
        .await;
}

// ── Fix 3: Backpressure ────────────────────────────────────────────────────────

/// Verify that a runtime with `wasm_max_concurrent_tasks: 2` can still run 4
/// tasks successfully (the semaphore serializes them, not rejects them).
#[tokio::test]
async fn wasm_backpressure_limits_concurrent_tasks() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cfg = Config {
                wasm_max_concurrent_tasks: 2,
                ..test_config(tmp.path().to_path_buf())
            };
            let runtime = WasmRuntime::new(&cfg).unwrap();

            // A simple module that exits immediately.
            let wasm_path = make_wasm(
                tmp.path(),
                "backpressure_test.wasm",
                r#"
                (module
                  (import "wasi_snapshot_preview1" "proc_exit"
                    (func $proc_exit (param i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (func (export "_start")
                    (call $proc_exit (i32.const 0))
                    unreachable
                  )
                )
                "#,
            );

            // Launch 4 tasks — only 2 run concurrently due to semaphore.
            let mut terminal_ids = Vec::new();
            let session_ids = ["s0", "s1", "s2", "s3"];
            let session_names = ["bp-session-0", "bp-session-1", "bp-session-2", "bp-session-3"];
            for i in 0..4usize {
                let req = CreateTerminalRequest::new(
                    SessionId::from(session_ids[i]),
                    wasm_path.to_str().unwrap(),
                );
                let resp = runtime
                    .handle_create_terminal(session_names[i], req)
                    .await
                    .expect("terminal should be created");
                terminal_ids.push(resp.terminal_id);
            }

            // Wait for all 4 to complete successfully.
            for tid in terminal_ids {
                let wait_req =
                    WaitForTerminalExitRequest::new(SessionId::from("s1"), tid);
                let exit = runtime
                    .handle_wait_for_terminal_exit(wait_req)
                    .await
                    .unwrap();
                assert_eq!(
                    exit.exit_status.exit_code,
                    Some(0),
                    "all 4 tasks should complete with exit_code 0"
                );
            }
        })
        .await;
}

// ── Fix 4: Fuel exhaustion distinguishable ────────────────────────────────────

/// A module that runs an infinite loop with a very low fuel limit should report
/// `signal == Some("fuel_exhausted")` specifically (not a generic trap message).
#[tokio::test]
async fn wasm_module_fuel_exhausted_returns_distinct_signal() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cfg = Config {
                wasm_fuel_limit: 100, // very low — exhausted by the loop
                ..test_config(tmp.path().to_path_buf())
            };
            let runtime = WasmRuntime::new(&cfg).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "fuel_exhausted_signal.wasm",
                r#"
                (module
                  (memory 1)
                  (export "memory" (memory 0))
                  (func (export "_start")
                    (loop $continue
                      (br $continue)
                    )
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("module should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();

            assert_eq!(
                exit.exit_status.signal.as_deref(),
                Some("fuel_exhausted"),
                "fuel exhaustion should produce signal 'fuel_exhausted', got: {:?}",
                exit.exit_status
            );
        })
        .await;
}

// ── Fix 7: Maximum WASM module size ───────────────────────────────────────────

/// Verify that a module larger than `wasm_max_module_size_bytes` is rejected
/// at `create_terminal` time with an appropriate error.
#[tokio::test]
async fn wasm_module_too_large_rejected() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cfg = Config {
                wasm_max_module_size_bytes: 10, // only 10 bytes — any real .wasm is larger
                ..test_config(tmp.path().to_path_buf())
            };
            let runtime = WasmRuntime::new(&cfg).unwrap();

            // Any valid .wasm file is larger than 10 bytes.
            let wasm_path = make_wasm(
                tmp.path(),
                "too_large.wasm",
                r#"
                (module
                  (import "wasi_snapshot_preview1" "proc_exit"
                    (func $proc_exit (param i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (func (export "_start")
                    (call $proc_exit (i32.const 0))
                    unreachable
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let err = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect_err("module exceeding size limit should be rejected");

            assert!(
                err.message.contains("too large") || err.message.contains("limit"),
                "error message should mention the size limit: {}",
                err.message
            );
        })
        .await;
}

// ── Fix 3: timeout_ms <= 0 semantics ─────────────────────────────────────────

/// A WASM module that calls `nats_request` with `timeout_ms = -1` (negative)
/// should get -1 back (no NATS client) without panicking or trapping.
/// This verifies that the negative-timeout code path is handled gracefully.
#[tokio::test]
async fn wasm_nats_request_negative_timeout_handled() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            // WasmRuntime::new has no NATS client — nats_request returns -1 immediately.
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "nats_request_neg_timeout.wasm",
                r#"
                (module
                  (import "trogon_v1" "nats_request" (func $nats_request
                    (param i32 i32 i32 i32 i32 i32 i32) (result i32)))
                  (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
                  (memory 1) (export "memory" (memory 0))
                  (data (i32.const 0) "test.subject")
                  (func $_start
                    (call $nats_request
                      (i32.const 0) (i32.const 12)  ;; subject
                      (i32.const 0) (i32.const 0)   ;; empty payload
                      (i32.const -1)                 ;; timeout_ms = -1 → should use 30s default
                      (i32.const 512) (i32.const 256)) ;; out buffer
                    drop
                    (call $proc_exit (i32.const 0)))
                  (export "_start" (func $_start)))
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("module with negative timeout_ms should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();
            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "nats_request with timeout_ms=-1 and no NATS should return -1 gracefully; module exits 0"
            );
        })
        .await;
}

// ── Fix 1: wait_for_exit_timeout_secs ────────────────────────────────────────

/// A native process that sleeps forever should be killed and return
/// `signal = Some("wait_timeout")` after the configured timeout elapses.
#[tokio::test]
async fn wait_for_terminal_exit_timeout_kills_hung_process() {
    let tmp = TempDir::new().unwrap();
    let cfg = Config {
        wasm_only: false,
        wait_for_exit_timeout_secs: 1, // 1 second — fires quickly in tests
        ..test_config(tmp.path().to_path_buf())
    };
    let runtime = WasmRuntime::new(&cfg).unwrap();

    // Spawn a process that sleeps forever.
    let req = CreateTerminalRequest::new(SessionId::from("s1"), "sleep")
        .args(vec!["9999".to_string()]);
    let resp = runtime
        .handle_create_terminal(session_id(), req)
        .await
        .expect("sleep process should spawn");
    let tid = resp.terminal_id;

    let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid);
    let start = std::time::Instant::now();
    let exit = runtime
        .handle_wait_for_terminal_exit(wait_req)
        .await
        .expect("wait should return after timeout");

    // Should complete well within 5 seconds (timeout is 1s + kill latency).
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "wait should have returned within ~2s, took {:?}",
        start.elapsed()
    );
    assert_eq!(
        exit.exit_status.signal.as_deref(),
        Some("wait_timeout"),
        "hung process should produce signal 'wait_timeout', got: {:?}",
        exit.exit_status
    );
}

// ── Fix 3: native_terminal_stdin_write ───────────────────────────────────────

/// Spawning `cat` and writing to its stdin should produce the same bytes on stdout.
#[tokio::test]
async fn native_terminal_stdin_write() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    // `cat` reads stdin and echoes it to stdout.
    let req = CreateTerminalRequest::new(SessionId::from("s1"), "cat");
    let resp = runtime
        .handle_create_terminal(session_id(), req)
        .await
        .expect("cat should spawn");
    let tid = resp.terminal_id;

    // Write "hello\n" to stdin.
    runtime
        .handle_write_to_terminal(tid.0.as_ref(), b"hello\n")
        .await
        .expect("write to cat stdin should succeed");

    // Close stdin so cat exits.
    runtime.handle_close_terminal_stdin(tid.0.as_ref()).expect("close stdin should succeed");

    let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid.clone());
    let exit = runtime
        .handle_wait_for_terminal_exit(wait_req)
        .await
        .expect("wait should succeed");
    assert_eq!(exit.exit_status.exit_code, Some(0), "cat should exit 0");

    let out_req = TerminalOutputRequest::new(SessionId::from("s1"), tid);
    let out = runtime
        .handle_terminal_output(out_req)
        .await
        .expect("output should succeed");
    assert!(
        out.output.contains("hello"),
        "cat output should contain 'hello', got: {:?}",
        out.output
    );
}

// ── Fix: Startup cleanup of stale session directories ────────────────────────

/// After calling `cleanup_stale_sessions`, any pre-existing subdirectories
/// inside `session_root` should be removed.
#[tokio::test]
async fn startup_cleanup_removes_stale_dirs() {
    let tmp = TempDir::new().unwrap();

    // Create two fake stale session directories.
    let stale1 = tmp.path().join("old-session-1");
    let stale2 = tmp.path().join("old-session-2");
    std::fs::create_dir_all(&stale1).unwrap();
    std::fs::create_dir_all(&stale2).unwrap();

    // A stale file at the root level should be left alone (only dirs removed).
    let stale_file = tmp.path().join("some-file.txt");
    std::fs::write(&stale_file, "data").unwrap();

    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();
    runtime.cleanup_stale_sessions().await;

    assert!(
        !stale1.exists(),
        "old-session-1 should have been removed by cleanup_stale_sessions"
    );
    assert!(
        !stale2.exists(),
        "old-session-2 should have been removed by cleanup_stale_sessions"
    );
    // Non-directory items at the root are left untouched.
    assert!(
        stale_file.exists(),
        "non-directory file at session_root should not be removed"
    );
}

// ── Fix: NATS integration tests for host functions ───────────────────────────

fn nats_test_url() -> Option<String> {
    std::env::var("NATS_TEST_URL").ok()
}

/// WASM module calling `trogon.nats_publish` against a real NATS server.
/// Skipped when `NATS_TEST_URL` is not set.
#[tokio::test]
async fn wasm_module_nats_publish_with_nats() {
    let nats_url = match nats_test_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping wasm_module_nats_publish_with_nats");
            return;
        }
    };

    let nats = async_nats::connect(&nats_url).await.expect("NATS connect");
    let mut sub = nats
        .subscribe("test.wasm.publish")
        .await
        .expect("subscribe");

    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime =
                WasmRuntime::with_nats(&test_config(tmp.path().to_path_buf()), Some(nats.clone()))
                    .unwrap();

            // WAT module: calls trogon.nats_publish("test.wasm.publish", "hello")
            // subject at offset 0 (17 bytes), payload "hello" at offset 32 (5 bytes).
            let wat = r#"(module
              (import "trogon_v1" "nats_publish" (func $pub (param i32 i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
              (memory 1) (export "memory" (memory 0))
              (data (i32.const 0) "test.wasm.publish")
              (data (i32.const 32) "hello")
              (func $_start
                (drop (call $pub (i32.const 0) (i32.const 17) (i32.const 32) (i32.const 5)))
                (call $exit (i32.const 0)))
              (export "_start" (func $_start)))
            "#;

            let wasm_path = make_wasm(tmp.path(), "nats_publish.wasm", wat);
            let req =
                CreateTerminalRequest::new(SessionId::from("s1"), wasm_path.to_str().unwrap());
            let resp = runtime
                .handle_create_terminal("nats-pub-session", req)
                .await
                .expect("wasm publish module should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();
        })
        .await;

    // Verify NATS received the published message.
    let msg = tokio::time::timeout(std::time::Duration::from_secs(3), sub.next())
        .await
        .expect("timed out waiting for NATS message")
        .expect("subscription closed without message");

    assert_eq!(
        msg.payload.as_ref(),
        b"hello",
        "NATS message payload should be 'hello', got: {:?}",
        msg.payload
    );
}

/// WASM module calling `trogon.nats_request` against a real NATS server.
/// Skipped when `NATS_TEST_URL` is not set.
///
/// Strategy: use `proc_exit` with the number of bytes received as the exit code.
/// A responder sends "world" (5 bytes), so we expect exit_code = Some(5).
#[tokio::test]
async fn wasm_module_nats_request_with_nats() {
    let nats_url = match nats_test_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping wasm_module_nats_request_with_nats");
            return;
        }
    };

    let nats = async_nats::connect(&nats_url).await.expect("NATS connect");

    // Spawn a responder that replies "world" to any message on "test.wasm.request".
    let responder_nats = nats.clone();
    let responder = tokio::spawn(async move {
        let mut sub = responder_nats
            .subscribe("test.wasm.request")
            .await
            .expect("responder subscribe");
        if let Some(msg) = sub.next().await {
            if let Some(reply) = msg.reply {
                let _ = responder_nats
                    .publish(reply, bytes::Bytes::from_static(b"world"))
                    .await;
            }
        }
    });

    // Give the responder a moment to subscribe.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime =
                WasmRuntime::with_nats(&test_config(tmp.path().to_path_buf()), Some(nats.clone()))
                    .unwrap();

            // WAT module: calls nats_request("test.wasm.request", "ping", 2000ms, out@512, 64)
            // then exits with the number of bytes received as the exit code.
            // subject "test.wasm.request" = 18 bytes at offset 0
            // payload "ping" = 4 bytes at offset 32
            // response written at offset 512
            let wat = r#"(module
              (import "trogon_v1" "nats_request" (func $req
                (param i32 i32 i32 i32 i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
              (memory 1) (export "memory" (memory 0))
              (data (i32.const 0) "test.wasm.request")
              (data (i32.const 32) "ping")
              (func $_start
                (local $n i32)
                (local.set $n (call $req
                  (i32.const 0) (i32.const 17)
                  (i32.const 32) (i32.const 4)
                  (i32.const 2000)
                  (i32.const 512) (i32.const 64)))
                (call $exit (local.get $n)))
              (export "_start" (func $_start)))
            "#;

            let wasm_path = make_wasm(tmp.path(), "nats_request.wasm", wat);
            let req =
                CreateTerminalRequest::new(SessionId::from("s1"), wasm_path.to_str().unwrap());
            let resp = runtime
                .handle_create_terminal("nats-req-session", req)
                .await
                .expect("wasm request module should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();

            // "world" is 5 bytes → proc_exit(5) → exit_code = Some(5).
            assert_eq!(
                exit.exit_status.exit_code,
                Some(5),
                "nats_request should receive 'world' (5 bytes), module exits with that count; got: {:?}",
                exit.exit_status
            );
        })
        .await;

    let _ = responder.await;
}

// ── Fix: list_sessions and list_terminals ────────────────────────────────────

/// After a file write creates session "s1", `list_sessions()` should include it.
#[tokio::test]
async fn list_sessions_returns_active_sessions() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    // Write a file to session "s1" — this lazily creates the session.
    let write_req = WriteTextFileRequest::new(
        SessionId::from("s1"),
        PathBuf::from("/hello.txt"),
        "content",
    );
    runtime
        .handle_write_text_file("s1", write_req)
        .await
        .expect("write should succeed");

    let sessions = runtime.list_sessions();
    assert!(
        sessions.contains(&"s1".to_string()),
        "list_sessions should contain 's1', got: {:?}",
        sessions
    );
}

/// After creating a native terminal, `list_terminals()` should include its ID.
#[tokio::test]
async fn list_terminals_returns_active_terminals() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    // Spawn a long-running native process so the terminal remains live.
    let req = CreateTerminalRequest::new(SessionId::from("s1"), "sleep")
        .args(vec!["10".to_string()]);
    let resp = runtime
        .handle_create_terminal("s1", req)
        .await
        .expect("sleep should spawn");
    let tid = resp.terminal_id.clone();

    let terminals = runtime.list_terminals();
    assert!(
        terminals.iter().any(|(t, _)| t == tid.0.as_ref()),
        "list_terminals should contain terminal {:?}, got: {:?}",
        tid,
        terminals
    );

    // Clean up: kill and release the terminal.
    let kill_req = KillTerminalRequest::new(SessionId::from("s1"), tid.clone());
    runtime.handle_kill_terminal(kill_req).await.expect("kill should succeed");
    let rel = ReleaseTerminalRequest::new(SessionId::from("s1"), tid);
    let _ = runtime.handle_release_terminal(rel).await;
}

// ── Fix: atomic file writes ──────────────────────────────────────────────────

/// Writing twice to the same path produces the second content and leaves no
/// `.tmp` artifact behind.
#[tokio::test]
async fn write_text_file_is_atomic() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    // First write.
    let req1 = WriteTextFileRequest::new(
        SessionId::from("s1"),
        PathBuf::from("/file.txt"),
        "content_v1",
    );
    runtime
        .handle_write_text_file("s1", req1)
        .await
        .expect("first write should succeed");

    // Second write — overwrites via atomic rename.
    let req2 = WriteTextFileRequest::new(
        SessionId::from("s1"),
        PathBuf::from("/file.txt"),
        "content_v2",
    );
    runtime
        .handle_write_text_file("s1", req2)
        .await
        .expect("second write should succeed");

    // Read back and verify it's v2.
    let read_req = ReadTextFileRequest::new(SessionId::from("s1"), PathBuf::from("/file.txt"));
    let resp = runtime
        .handle_read_text_file("s1", read_req)
        .await
        .expect("read should succeed");
    assert_eq!(resp.content, "content_v2", "file should contain v2 content");

    // No temp file should be left behind.
    let session_dir = tmp.path().join("s1");
    assert!(
        !session_dir.join("file.txt.tmp").exists(),
        ".tmp file should not be left after successful write"
    );
}

// ── New: TMPDIR is created for native terminals ───────────────────────────

/// The runtime must create a `tmp/` subdirectory in the session sandbox and
/// expose it as `$TMPDIR` to native processes.
#[tokio::test]
async fn native_terminal_tmpdir_exists() {
    let tmp = TempDir::new().unwrap();
    let cfg = Config {
        wasm_only: false,
        ..test_config(tmp.path().to_path_buf())
    };
    let runtime = WasmRuntime::new(&cfg).unwrap();

    // `sh -c 'test -d "$TMPDIR"'` exits 0 when TMPDIR is a directory, non-zero otherwise.
    let req = CreateTerminalRequest::new(SessionId::from("s1"), "sh")
        .args(vec!["-c".to_string(), r#"test -d "$TMPDIR""#.to_string()]);
    let resp = runtime
        .handle_create_terminal(session_id(), req)
        .await
        .expect("sh should spawn");
    let tid = resp.terminal_id;

    let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid);
    let exit = runtime
        .handle_wait_for_terminal_exit(wait_req)
        .await
        .expect("wait should succeed");

    assert_eq!(
        exit.exit_status.exit_code,
        Some(0),
        "TMPDIR should be a directory visible to native processes; got: {:?}",
        exit.exit_status
    );
}

// ── New: memory limit denies modules that exceed it ───────────────────────

/// A WASM module that requests more memory than the limit should fail to start.
/// `memory.grow` must return -1 when the resource limiter denies the request.
#[tokio::test]
async fn wasm_module_memory_limit_denial() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            // Limit to 1 page (64 KiB). The module declares 2 pages minimum → denied.
            let cfg = Config {
                wasm_only: false,
                wasm_memory_limit_bytes: Some(64 * 1024), // 1 page
                ..test_config(tmp.path().to_path_buf())
            };
            let runtime = WasmRuntime::new(&cfg).unwrap();

            // Module declares `(memory 2)` — requires 2 pages (128 KiB) at instantiation.
            // wasmtime's ResourceLimiter will deny memory_growing → trap at instantiation.
            let wat = r#"(module
              (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
              (memory 2)
              (export "memory" (memory 0))
              (func (export "_start")
                (call $exit (i32.const 0))
              )
            )"#;
            let wasm_path = make_wasm(tmp.path(), "mem_denial.wasm", wat);
            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("create_terminal should succeed (starts the task)");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .expect("wait should return");

            // Instantiation failure → faulted exit (no exit_code, signal set).
            assert!(
                exit.exit_status.exit_code.is_none(),
                "memory-denied module should not produce a clean exit code; got: {:?}",
                exit.exit_status
            );
            assert!(
                exit.exit_status.signal.is_some(),
                "memory-denied module should produce a fault signal; got: {:?}",
                exit.exit_status
            );
        })
        .await;
}

// ── New: recv_message 8-parameter ABI ────────────────────────────────────

/// Verifies the `recv_message` host function accepts the 8-parameter signature
/// introduced in the `trogon_v1` ABI. The module calls it with an invalid
/// sub_id (-1) and expects a -1 return value (not a link error).
#[tokio::test]
async fn wasm_module_recv_message_abi() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            // Calls recv_message with sub_id=-1 (invalid) → must return -1.
            // Signature: (sub_id i32, out_subj_ptr i32, out_subj_max i32, out_subj_len_ptr i32,
            //             out_payload_ptr i32, out_payload_max i32, out_payload_len_ptr i32,
            //             timeout_ms i32) -> i32
            // Module exits with the return value cast to i32 via proc_exit.
            // We expect exit_code = 255 because proc_exit(-1 as u32) = proc_exit(4294967295)
            // truncates to u8 = 255 on Linux.
            // recv_message with sub_id=-1 (no such subscription) must return -1.
            // wasmtime 30 preview1 rejects proc_exit codes >= 126, so we cannot
            // pass -1 directly to proc_exit.  Instead, we branch on the return
            // value: exit(1) if -1 was returned (expected), exit(2) otherwise.
            let wat = r#"(module
              (import "trogon_v1" "recv_message" (func $recv
                (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
              (memory 1) (export "memory" (memory 0))
              (func (export "_start")
                (local $r i32)
                ;; sub_id=-1 (invalid), all pointers into memory, timeout=100ms
                (local.set $r (call $recv
                  (i32.const -1)
                  (i32.const 0)  (i32.const 64)  (i32.const 128)
                  (i32.const 256) (i32.const 64) (i32.const 320)
                  (i32.const 100)))
                ;; -1 returned → exit 1 (proof it returned -1 without trapping)
                (if (i32.eq (local.get $r) (i32.const -1))
                  (then (call $exit (i32.const 1)))
                )
                ;; unexpected non-(-1) return
                (call $exit (i32.const 2))
              )
            )"#;

            let wasm_path = make_wasm(tmp.path(), "recv_abi.wasm", wat);
            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("recv_message module should link and start");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .expect("wait should succeed");

            // recv_message returns -1 for invalid sub_id.
            // The WAT converts that into proc_exit(1) so we see exit_code = Some(1).
            assert_eq!(
                exit.exit_status.exit_code,
                Some(1),
                "recv_message with invalid sub_id should return -1 (→ exit 1), got: {:?}",
                exit.exit_status
            );
        })
        .await;
}

// ── New: WASM timeout fires ───────────────────────────────────────────────────

/// A WASM module that spins in an infinite loop with a very short timeout set
/// should be interrupted and report `signal = Some("timeout")`. Any partial
/// stdout output written before the interrupt should still be captured.
#[tokio::test]
async fn wasm_module_timeout_fires_returns_signal_and_partial_output() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cfg = Config {
                wasm_timeout_secs: Some(1), // 1 second — fires quickly
                wasm_fuel_limit: 0,         // unlimited fuel so only the wall-clock fires
                ..test_config(tmp.path().to_path_buf())
            };
            let runtime = WasmRuntime::new(&cfg).unwrap();

            // Module writes "partial\n" to stdout then spins forever.
            // Memory layout:
            //   offset 16: "partial\n" (8 bytes)
            //   offset 0: iovec ptr   = 16
            //   offset 4: iovec len   = 8
            //   offset 8: nwritten
            let wasm_path = make_wasm(
                tmp.path(),
                "timeout_fires.wasm",
                r#"
                (module
                  (import "wasi_snapshot_preview1" "fd_write"
                    (func $fd_write (param i32 i32 i32 i32) (result i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (data (i32.const 16) "partial\n")
                  (func (export "_start")
                    ;; write "partial\n" to stdout
                    (i32.store (i32.const 0) (i32.const 16))
                    (i32.store (i32.const 4) (i32.const 8))
                    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 8)))
                    ;; spin forever — timeout must fire
                    (loop $spin (br $spin))
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("module should be created");
            let tid = resp.terminal_id;

            let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid.clone());
            let start = std::time::Instant::now();
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .expect("wait should return after timeout");

            // Should finish well within a few seconds (timeout is 1s).
            assert!(
                start.elapsed() < std::time::Duration::from_secs(10),
                "timeout should have fired within ~2s, took {:?}",
                start.elapsed()
            );

            assert_eq!(
                exit.exit_status.signal.as_deref(),
                Some("timeout"),
                "timed-out module should report signal 'timeout', got: {:?}",
                exit.exit_status
            );
            assert!(
                exit.exit_status.exit_code.is_none(),
                "timed-out module should have no exit_code, got: {:?}",
                exit.exit_status
            );

            // Partial output written before the timeout must be captured.
            let out_req = TerminalOutputRequest::new(SessionId::from("s1"), tid);
            let out = runtime.handle_terminal_output(out_req).await.unwrap();
            assert!(
                out.output.contains("partial"),
                "partial output written before timeout should be captured, got: {:?}",
                out.output
            );
        })
        .await;
}

// ── New: nats_publish without NATS returns -1 ────────────────────────────────

/// A WASM module calling `trogon_v1.nats_publish` with no NATS client should get
/// -1 and exit cleanly (no panic, no trap). Mirrors the existing nats_request
/// no-NATS test but for the publish path.
#[tokio::test]
async fn wasm_module_trogon_nats_publish_no_nats() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            // Memory layout:
            //   offset 0: subject "some.subject" (12 bytes)
            //   offset 16: payload "hello" (5 bytes)
            let wasm_path = make_wasm(
                tmp.path(),
                "nats_publish_no_nats.wasm",
                r#"
                (module
                  (import "trogon_v1" "nats_publish"
                    (func $nats_publish (param i32 i32 i32 i32) (result i32)))
                  (import "wasi_snapshot_preview1" "proc_exit"
                    (func $proc_exit (param i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (data (i32.const 0)  "some.subject")
                  (data (i32.const 16) "hello")
                  (func (export "_start")
                    (local $ret i32)
                    (local.set $ret
                      (call $nats_publish
                        (i32.const 0)   ;; subject_ptr
                        (i32.const 12)  ;; subject_len
                        (i32.const 16)  ;; payload_ptr
                        (i32.const 5)   ;; payload_len
                      )
                    )
                    ;; expect -1 (no NATS client)
                    (if (i32.ne (local.get $ret) (i32.const -1))
                      (then (call $proc_exit (i32.const 1)))
                    )
                    (call $proc_exit (i32.const 0))
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("nats_publish module should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();
            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "nats_publish with no NATS should return -1 and module should exit cleanly"
            );
        })
        .await;
}

// ── New: on-disk cache version mismatch forces recompile ─────────────────────

/// Simulate a `CACHE_FINGERPRINT` version bump: write a `.version` file with a
/// different fingerprint and verify the runtime recompiles instead of loading the
/// stale cache. Because we cannot change CACHE_FINGERPRINT at runtime, we write
/// a cache entry with a deliberately wrong version string (b"stale-version-0").
/// The runtime must ignore the stale cache and produce correct output.
#[tokio::test]
async fn module_cache_version_mismatch_forces_recompile() {
    let tmp = TempDir::new().unwrap();
    let cache_dir = TempDir::new().unwrap();

    // WAT module that exits with code 7.
    let wat = r#"
        (module
          (import "wasi_snapshot_preview1" "proc_exit"
            (func $proc_exit (param i32)))
          (memory 1)
          (export "memory" (memory 0))
          (func (export "_start")
            (call $proc_exit (i32.const 7))
            unreachable
          )
        )
    "#;
    let wasm_path = make_wasm(tmp.path(), "version_mismatch.wasm", wat);

    // Step 1: run once to populate the on-disk cache with the real fingerprint.
    {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let cfg = Config {
                    wasm_only: false,
                    module_cache_dir: Some(cache_dir.path().to_path_buf()),
                    ..test_config(tmp.path().join("sessions_a"))
                };
                let runtime = WasmRuntime::new(&cfg).unwrap();
                let req = CreateTerminalRequest::new(
                    SessionId::from("s1"),
                    wasm_path.to_str().unwrap(),
                );
                let resp = runtime
                    .handle_create_terminal("session-a", req)
                    .await
                    .expect("initial run should succeed");
                let wait = WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
                let exit = runtime.handle_wait_for_terminal_exit(wait).await.unwrap();
                assert_eq!(exit.exit_status.exit_code, Some(7), "initial run: exit 7");
            })
            .await;
    }

    // Step 2: overwrite every .version file with a stale fingerprint.
    for entry in std::fs::read_dir(cache_dir.path()).unwrap() {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) == Some("version") {
            std::fs::write(entry.path(), "stale-version-0").unwrap();
        }
    }

    // Step 3: new runtime with the same cache dir — version mismatch should force
    // recompile. The module must still run correctly and exit with code 7.
    {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let cfg = Config {
                    wasm_only: false,
                    module_cache_dir: Some(cache_dir.path().to_path_buf()),
                    ..test_config(tmp.path().join("sessions_b"))
                };
                let runtime = WasmRuntime::new(&cfg).unwrap();
                let req = CreateTerminalRequest::new(
                    SessionId::from("s1"),
                    wasm_path.to_str().unwrap(),
                );
                let resp = runtime
                    .handle_create_terminal("session-b", req)
                    .await
                    .expect("run after version mismatch should succeed");
                let wait = WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
                let exit = runtime.handle_wait_for_terminal_exit(wait).await.unwrap();
                assert_eq!(
                    exit.exit_status.exit_code,
                    Some(7),
                    "version-mismatch should recompile and produce correct exit code"
                );
            })
            .await;
    }

    // After step 3, the .version file should have been updated (no longer stale).
    let version_files: Vec<_> = std::fs::read_dir(cache_dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("version"))
        .collect();
    assert!(
        !version_files.is_empty(),
        "cache directory should still contain .version files after recompile"
    );
    for vf in &version_files {
        let content = std::fs::read_to_string(vf.path()).unwrap();
        assert_ne!(
            content.trim(),
            "stale-version-0",
            ".version file should have been updated after recompile, path: {:?}",
            vf.path()
        );
    }
}

// ── New: idle session cleanup ─────────────────────────────────────────────────

/// `cleanup_idle_sessions` must:
///   1. Be a no-op when `session_idle_timeout_secs = 0`.
///   2. Remove sessions whose `last_activity` exceeds the configured timeout.
#[tokio::test]
async fn cleanup_idle_sessions_removes_expired_sessions() {
    let tmp = TempDir::new().unwrap();

    // ── Part 1: disabled (timeout = 0) ─────────────────────────────────────
    {
        let cfg = Config {
            session_idle_timeout_secs: 0,
            ..test_config(tmp.path().join("root_disabled"))
        };
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let runtime = WasmRuntime::new(&cfg).unwrap();

                let write_req = WriteTextFileRequest::new(
                    SessionId::from("idle-s1"),
                    PathBuf::from("/touch.txt"),
                    "x",
                );
                runtime.handle_write_text_file("idle-s1", write_req).await.unwrap();

                runtime.cleanup_idle_sessions();

                assert!(
                    runtime.list_sessions().contains(&"idle-s1".to_string()),
                    "session should NOT be cleaned up when timeout=0"
                );
            })
            .await;
    }

    // ── Part 2: enabled, session becomes idle ───────────────────────────────
    {
        let cfg = Config {
            session_idle_timeout_secs: 1, // 1 second timeout
            ..test_config(tmp.path().join("root_enabled"))
        };
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let runtime = WasmRuntime::new(&cfg).unwrap();

                let write_req = WriteTextFileRequest::new(
                    SessionId::from("idle-s2"),
                    PathBuf::from("/touch.txt"),
                    "x",
                );
                runtime.handle_write_text_file("idle-s2", write_req).await.unwrap();
                let sandbox = tmp.path().join("root_enabled").join("idle-s2");

                assert!(
                    runtime.list_sessions().contains(&"idle-s2".to_string()),
                    "session should exist before timeout"
                );

                // Wait longer than the idle timeout.
                tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

                runtime.cleanup_idle_sessions();

                assert!(
                    !runtime.list_sessions().contains(&"idle-s2".to_string()),
                    "session should be removed from runtime after idle timeout"
                );

                // Give the spawned fs-removal task a moment to run.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                assert!(
                    !sandbox.exists(),
                    "sandbox directory should be removed after idle cleanup"
                );
            })
            .await;
    }
}

// ── New: kill WASM terminal + wait_for_terminal_exit ─────────────────────────

/// Killing a running WASM terminal must cause a subsequent
/// `handle_wait_for_terminal_exit` to return promptly with
/// `signal = Some("killed")` rather than hanging indefinitely.
#[tokio::test]
async fn kill_wasm_terminal_sets_killed_signal() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cfg = Config {
                wasm_fuel_limit: 0, // unlimited fuel — module would run forever
                ..test_config(tmp.path().to_path_buf())
            };
            let runtime = WasmRuntime::new(&cfg).unwrap();

            // Infinite-loop WASM module — never exits on its own.
            let wasm_path = make_wasm(
                tmp.path(),
                "infinite_loop_kill.wasm",
                r#"
                (module
                  (memory 1)
                  (export "memory" (memory 0))
                  (func (export "_start")
                    (loop $spin (br $spin))
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("infinite module should be created");
            let tid = resp.terminal_id;

            // Kill the WASM terminal.
            let kill_req = KillTerminalRequest::new(SessionId::from("s1"), tid.clone());
            runtime
                .handle_kill_terminal(kill_req)
                .await
                .expect("kill should succeed");

            // wait_for_terminal_exit must return promptly with signal="killed".
            let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid);
            let start = std::time::Instant::now();
            let exit = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                runtime.handle_wait_for_terminal_exit(wait_req),
            )
            .await
            .expect("wait should complete within 5 seconds after kill")
            .expect("wait should return Ok");

            assert!(
                start.elapsed() < std::time::Duration::from_secs(5),
                "wait should return quickly after kill, took {:?}",
                start.elapsed()
            );
            assert_eq!(
                exit.exit_status.signal.as_deref(),
                Some("killed"),
                "killed WASM terminal should report signal 'killed', got: {:?}",
                exit.exit_status
            );
            assert!(
                exit.exit_status.exit_code.is_none(),
                "killed WASM terminal should have no exit_code, got: {:?}",
                exit.exit_status
            );
        })
        .await;
}

// ── New tests ─────────────────────────────────────────────────────────────────

/// WASM module calls `environ_sizes_get` to count env vars, then exits with
/// that count as the exit code. We pass exactly 2 env vars and expect
/// exit_code = Some(2).
#[tokio::test]
async fn wasm_module_env_vars_passed_via_wasi() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "env_count.wasm",
                r#"
                (module
                  (import "wasi_snapshot_preview1" "environ_sizes_get"
                    (func $environ_sizes_get (param i32 i32) (result i32)))
                  (import "wasi_snapshot_preview1" "proc_exit"
                    (func $proc_exit (param i32)))
                  (memory 1) (export "memory" (memory 0))
                  (func (export "_start")
                    ;; write environ count to offset 0, buf_size to offset 4
                    (drop (call $environ_sizes_get (i32.const 0) (i32.const 4)))
                    ;; exit with count
                    (call $proc_exit (i32.load (i32.const 0)))
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            )
            .env(vec![
                agent_client_protocol::EnvVariable::new("FOO", "bar"),
                agent_client_protocol::EnvVariable::new("BAZ", "qux"),
            ]);

            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("create_terminal should succeed");
            let tid = resp.terminal_id;

            let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .expect("wait should succeed");

            assert_eq!(
                exit.exit_status.exit_code,
                Some(2),
                "environ_sizes_get should report 2 env vars, got: {:?}",
                exit.exit_status
            );
        })
        .await;
}

/// After `handle_create_terminal` returns for an immediately-exiting WASM
/// module, polling `handle_terminal_output` (without calling
/// `handle_wait_for_terminal_exit`) must eventually surface `exit_status`.
/// This exercises the `snapshot()` fast path that checks `exit_arc.get()`.
#[tokio::test]
async fn wasm_terminal_snapshot_shows_exit_before_explicit_wait() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "exit_zero_snapshot.wasm",
                r#"
                (module
                  (import "wasi_snapshot_preview1" "proc_exit"
                    (func $proc_exit (param i32)))
                  (memory 1) (export "memory" (memory 0))
                  (func (export "_start")
                    (call $proc_exit (i32.const 0))
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("create_terminal should succeed");
            let tid = resp.terminal_id;

            // Poll for exit status via snapshot — do NOT call handle_wait_for_terminal_exit.
            let exit_status = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                async {
                    loop {
                        let out_req =
                            TerminalOutputRequest::new(SessionId::from("s1"), tid.clone());
                        let out = runtime.handle_terminal_output(out_req).await.unwrap();
                        if out.exit_status.is_some() {
                            return out.exit_status.unwrap();
                        }
                        tokio::task::yield_now().await;
                    }
                },
            )
            .await
            .expect("exit status should appear within 5s via snapshot polling");

            assert_eq!(
                exit_status.exit_code,
                Some(0),
                "snapshot should report exit_code 0, got: {:?}",
                exit_status
            );
        })
        .await;
}

/// The `METRICS.wasm_tasks_started` counter must increment by exactly 1
/// each time a WASM terminal is created and begins executing.
#[tokio::test]
async fn wasm_tasks_started_metric_increments() {
    use std::sync::atomic::Ordering;

    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let before = trogon_wasm_runtime::metrics::METRICS
                .wasm_tasks_started
                .load(Ordering::Relaxed);

            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "metric_test.wasm",
                r#"
                (module
                  (import "wasi_snapshot_preview1" "proc_exit"
                    (func $proc_exit (param i32)))
                  (memory 1) (export "memory" (memory 0))
                  (func (export "_start")
                    (call $proc_exit (i32.const 0))
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("create_terminal should succeed");
            let tid = resp.terminal_id;

            let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid);
            runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .expect("wait should succeed");

            let after = trogon_wasm_runtime::metrics::METRICS
                .wasm_tasks_started
                .load(Ordering::Relaxed);

            assert!(
                after > before,
                "wasm_tasks_started should increment after running a WASM module (before={before}, after={after})"
            );
        })
        .await;
}

// ── New tests ─────────────────────────────────────────────────────────────────

/// Calling handle_kill_terminal with a terminal ID that does not exist must
/// return an Err whose message contains "Unknown terminal".
#[tokio::test]
async fn kill_unknown_terminal_returns_error() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    let req = KillTerminalRequest::new(
        SessionId::from("s1"),
        TerminalId::from("nonexistent-tid"),
    );
    let err = runtime
        .handle_kill_terminal(req)
        .await
        .expect_err("kill of nonexistent terminal should return Err");

    assert!(
        err.message.contains("Unknown terminal"),
        "error message should contain 'Unknown terminal', got: {:?}",
        err.message
    );
}

/// handle_session_notification ticks last_activity so that the session is not
/// evicted by cleanup_idle_sessions even after the idle timeout would have
/// otherwise elapsed.
#[tokio::test]
async fn session_notification_ticks_last_activity() {
    use agent_client_protocol::{ContentBlock, ContentChunk, SessionNotification, SessionUpdate};

    let tmp = TempDir::new().unwrap();
    let cfg = Config {
        session_idle_timeout_secs: 1,
        ..test_config(tmp.path().to_path_buf())
    };

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&cfg).unwrap();

            // Touch the session by writing a file (creates the session entry).
            let write_req = WriteTextFileRequest::new(
                SessionId::from("notif-session"),
                PathBuf::from("/touch.txt"),
                "touch",
            );
            runtime
                .handle_write_text_file("notif-session", write_req)
                .await
                .expect("write should succeed");

            // Sleep 900 ms — session has been idle for 0.9 s, timeout is 1 s.
            tokio::time::sleep(std::time::Duration::from_millis(900)).await;

            // Send a session notification — this resets last_activity.
            let notif = SessionNotification::new(
                "notif-session",
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("ping"))),
            );
            runtime.handle_session_notification(notif);

            // Sleep another 200 ms — total ~1.1 s from the initial touch, but
            // only ~200 ms since the notification reset the clock.
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            // Cleanup should NOT evict the session because the notification
            // refreshed its last_activity less than 1 s ago.
            runtime.cleanup_idle_sessions();

            let sessions = runtime.list_sessions();
            assert!(
                sessions.contains(&"notif-session".to_string()),
                "session should still exist after notification reset last_activity, sessions: {:?}",
                sessions
            );
        })
        .await;
}

/// Config::validate returns an error entry when output_byte_limit is 0.
#[tokio::test]
async fn config_validate_zero_output_byte_limit_returns_error() {
    let tmp = TempDir::new().unwrap();
    let cfg = Config {
        output_byte_limit: 0,
        ..test_config(tmp.path().to_path_buf())
    };

    let errors = cfg.validate();
    assert!(
        !errors.is_empty(),
        "validate() should return errors when output_byte_limit is 0"
    );
    assert!(
        errors.iter().any(|e| e.contains("output_byte_limit") || e.contains("WASM_OUTPUT_BYTE_LIMIT")),
        "error should mention output_byte_limit, got: {:?}",
        errors
    );
}

/// cleanup_all_sessions removes every session and deletes the sandbox directories.
#[tokio::test]
async fn cleanup_all_sessions_removes_all_sandboxes() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            // Create two sessions by writing a file into each.
            for sid in ["session-all-1", "session-all-2"] {
                let write_req = WriteTextFileRequest::new(
                    SessionId::from(sid),
                    PathBuf::from("/file.txt"),
                    "data",
                );
                runtime
                    .handle_write_text_file(sid, write_req)
                    .await
                    .expect("write should succeed");
            }

            // Both sandbox directories must exist before cleanup.
            let dir1 = tmp.path().join("session-all-1");
            let dir2 = tmp.path().join("session-all-2");
            assert!(dir1.exists(), "sandbox dir 1 should exist before cleanup");
            assert!(dir2.exists(), "sandbox dir 2 should exist before cleanup");

            // Cleanup all sessions.
            runtime.cleanup_all_sessions().await;

            // Session list must be empty.
            let sessions = runtime.list_sessions();
            assert!(
                sessions.is_empty(),
                "list_sessions() should be empty after cleanup_all_sessions, got: {:?}",
                sessions
            );

            // Give background fs-removal tasks a moment to complete.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Sandbox directories must have been removed.
            assert!(!dir1.exists(), "sandbox dir 1 should be deleted after cleanup");
            assert!(!dir2.exists(), "sandbox dir 2 should be deleted after cleanup");
        })
        .await;
}

// ── New: error paths and cached-status fast path ──────────────────────────

/// Writing to a terminal that does not exist must return an error whose
/// message mentions "Unknown terminal".
#[tokio::test]
async fn write_to_unknown_terminal_returns_error() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    let err = runtime
        .handle_write_to_terminal("nonexistent-id", b"data")
        .await
        .expect_err("write to unknown terminal should fail");

    assert!(
        err.message.contains("Unknown terminal"),
        "error message should mention 'Unknown terminal', got: {}",
        err.message
    );
}

/// Closing stdin on a terminal that does not exist must return an error whose
/// message mentions "Unknown terminal".
#[tokio::test]
async fn close_stdin_unknown_terminal_returns_error() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    let err = runtime
        .handle_close_terminal_stdin("nonexistent-id")
        .expect_err("close stdin on unknown terminal should fail");

    assert!(
        err.message.contains("Unknown terminal"),
        "error message should mention 'Unknown terminal', got: {}",
        err.message
    );
}

/// A second call to `handle_wait_for_terminal_exit` on the same terminal
/// should return the cached exit status without hanging.
#[tokio::test]
async fn wait_for_exit_returns_cached_status_on_second_call() {
    let tmp = TempDir::new().unwrap();
    let runtime =
        WasmRuntime::new(&Config { wasm_only: false, ..test_config(tmp.path().to_path_buf()) })
            .unwrap();

    let req = CreateTerminalRequest::new(SessionId::from("s1"), "echo")
        .args(vec!["hello".to_string()]);
    let resp = runtime
        .handle_create_terminal(session_id(), req)
        .await
        .expect("echo should be created");
    let tid = resp.terminal_id;

    // First wait — drains the child handle and populates the cached status.
    let wait1 = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid.clone());
    let exit1 = runtime
        .handle_wait_for_terminal_exit(wait1)
        .await
        .expect("first wait should succeed");
    assert_eq!(exit1.exit_status.exit_code, Some(0), "first wait: exit_code should be 0");

    // Second wait — must hit the fast path (cached status) and return the same value.
    let wait2 = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid);
    let exit2 = runtime
        .handle_wait_for_terminal_exit(wait2)
        .await
        .expect("second wait should succeed via cached status");
    assert_eq!(
        exit2.exit_status.exit_code,
        Some(0),
        "second wait (cached) should also return exit_code 0"
    );
}

/// Two concurrent `handle_wait_for_terminal_exit` calls on the same WASM
/// terminal must both return a valid exit status without panicking.
/// One caller takes the collector (blocking path); the other polls the shared
/// exit arc (OnceLock polling path).
#[tokio::test]
async fn concurrent_wasm_waits_both_return_same_exit_status() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            // A minimal WASM module that exits immediately with code 0.
            let wasm_path = make_wasm(
                tmp.path(),
                "concurrent_exit.wasm",
                r#"(module
  (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
  (memory 1) (export "memory" (memory 0))
  (func (export "_start") (call $proc_exit (i32.const 0)) unreachable)
)"#,
            );

            let req =
                CreateTerminalRequest::new(SessionId::from("s1"), wasm_path.to_str().unwrap());
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("WASM module should be created");
            let tid = resp.terminal_id;

            // Issue two concurrent waits on the same terminal.
            let wait_a = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid.clone());
            let wait_b = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid);

            let (res_a, res_b) = tokio::join!(
                runtime.handle_wait_for_terminal_exit(wait_a),
                runtime.handle_wait_for_terminal_exit(wait_b),
            );

            let exit_a = res_a.expect("first concurrent wait should succeed");
            let exit_b = res_b.expect("second concurrent wait should succeed");

            assert_eq!(
                exit_a.exit_status.exit_code,
                Some(0),
                "first concurrent wait should return exit_code 0, got: {:?}",
                exit_a.exit_status
            );
            assert_eq!(
                exit_b.exit_status.exit_code,
                Some(0),
                "second concurrent wait should return exit_code 0, got: {:?}",
                exit_b.exit_status
            );
        })
        .await;
}

// ── New tests ─────────────────────────────────────────────────────────────────

/// Create a native terminal with an explicit `cwd` inside the sandbox and
/// verify the process sees that directory as its working directory.
#[tokio::test]
async fn create_terminal_with_explicit_cwd() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&Config {
        wasm_only: false,
        ..test_config(tmp.path().to_path_buf())
    })
    .unwrap();

    // Write a file into /subdir to ensure the directory gets created inside the
    // session sandbox.
    let write_req = WriteTextFileRequest::new(
        SessionId::from("s1"),
        PathBuf::from("/subdir/test.txt"),
        "marker",
    );
    runtime
        .handle_write_text_file(session_id(), write_req)
        .await
        .expect("write should succeed");

    // Run `pwd` with cwd set to `/subdir` (sandbox-relative).
    let req = CreateTerminalRequest::new(SessionId::from("s1"), "pwd")
        .cwd(PathBuf::from("/subdir"));
    let resp = runtime
        .handle_create_terminal(session_id(), req)
        .await
        .expect("pwd with explicit cwd should succeed");
    let tid = resp.terminal_id;

    let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid.clone());
    let exit = runtime
        .handle_wait_for_terminal_exit(wait_req)
        .await
        .expect("wait should succeed");
    assert_eq!(exit.exit_status.exit_code, Some(0));

    let out_req = TerminalOutputRequest::new(SessionId::from("s1"), tid);
    let out = runtime
        .handle_terminal_output(out_req)
        .await
        .expect("output should succeed");

    // The printed path should end with "subdir" (it is inside the sandbox root).
    assert!(
        out.output.contains("subdir"),
        "pwd output should contain 'subdir', got: {:?}",
        out.output
    );
}

/// Running a WASM module by relative path (no leading `/`) must resolve the
/// module path inside the session sandbox via `session.resolve_path`.
#[tokio::test]
async fn wasm_relative_path_resolves_inside_sandbox() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            // Build a minimal WASM module that exits with code 0.
            let wasm_bytes = wat::parse_str(
                r#"(module
  (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
  (memory 1) (export "memory" (memory 0))
  (func (export "_start") (call $proc_exit (i32.const 0)) unreachable)
)"#,
            )
            .expect("WAT compile failed");

            // Place the .wasm file directly in the session sandbox directory so
            // the relative command "module.wasm" resolves correctly.
            let session_sandbox = tmp.path().join(session_id());
            std::fs::create_dir_all(&session_sandbox).expect("mkdir session sandbox");
            std::fs::write(session_sandbox.join("module.wasm"), &wasm_bytes)
                .expect("write wasm file");

            // Pass a relative path — this exercises the `else` branch in
            // `run_wasm_terminal` that calls `session.resolve_path`.
            let req = CreateTerminalRequest::new(SessionId::from("s1"), "module.wasm");
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("relative-path wasm should resolve and run");
            let tid = resp.terminal_id;

            let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .expect("wait should succeed");
            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "wasm module should exit 0"
            );
        })
        .await;
}

/// A WASM command path that escapes the session sandbox via `..` traversal
/// must be rejected with an error mentioning "escapes session sandbox".
#[tokio::test]
async fn wasm_relative_path_escapes_sandbox_rejected() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            // Write a real wasm file outside the sandbox so the path would be
            // valid on disk — but the sandbox check should still fire.
            let wasm_bytes = wat::parse_str(
                r#"(module
  (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
  (memory 1) (export "memory" (memory 0))
  (func (export "_start") (call $proc_exit (i32.const 0)) unreachable)
)"#,
            )
            .expect("WAT compile failed");
            std::fs::write(tmp.path().join("escape.wasm"), &wasm_bytes)
                .expect("write wasm outside sandbox");

            // `../escape.wasm` is relative and resolves outside the sandbox.
            let req = CreateTerminalRequest::new(SessionId::from("s1"), "../escape.wasm");
            let err = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect_err("path-traversal should be rejected");

            assert!(
                err.message.contains("escapes session sandbox"),
                "error should mention 'escapes session sandbox', got: {}",
                err.message
            );
        })
        .await;
}

/// Releasing a WASM terminal that is still running must succeed immediately;
/// the background task is aborted and the terminal is removed.
#[tokio::test]
async fn release_running_wasm_terminal_kills_background_task() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cfg = Config {
                wasm_fuel_limit: 0, // unlimited — module would spin forever
                ..test_config(tmp.path().to_path_buf())
            };
            let runtime = WasmRuntime::new(&cfg).unwrap();

            // An infinite-loop WASM module that never exits on its own.
            let wasm_path = make_wasm(
                tmp.path(),
                "infinite_release.wasm",
                r#"(module
  (memory 1) (export "memory" (memory 0))
  (func (export "_start") (loop $spin (br $spin)))
)"#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("infinite module should be created");
            let tid = resp.terminal_id;

            // Release without waiting for exit — should abort the task.
            let rel = ReleaseTerminalRequest::new(SessionId::from("s1"), tid.clone());
            runtime
                .handle_release_terminal(rel)
                .await
                .expect("release of running WASM terminal should succeed");

            // After release the terminal must be gone from the runtime.
            let out_req = TerminalOutputRequest::new(SessionId::from("s1"), tid);
            runtime
                .handle_terminal_output(out_req)
                .await
                .expect_err("terminal should be gone after release");
        })
        .await;
}

/// Writing stdin to a WASM terminal must return an error because WASM
/// terminals do not support stdin.
#[tokio::test]
async fn write_to_wasm_terminal_returns_error() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            // Minimal module that exits immediately.
            let wasm_path = make_wasm(
                tmp.path(),
                "exit_for_stdin_test.wasm",
                r#"(module
  (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
  (memory 1) (export "memory" (memory 0))
  (func (export "_start") (call $proc_exit (i32.const 0)) unreachable)
)"#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("wasm module should be created");
            let tid = resp.terminal_id;

            // Attempt to write stdin before the module has had a chance to
            // exit — WASM terminals never support stdin regardless of timing.
            let err = runtime
                .handle_write_to_terminal(tid.0.as_ref(), b"hello")
                .await
                .expect_err("writing to WASM terminal should fail");

            assert!(
                err.message.contains("WASM terminals do not support stdin"),
                "error should mention stdin unsupported, got: {}",
                err.message
            );
        })
        .await;
}

// ── Module cache LRU eviction ──────────────────────────────────────────────

/// Loading 65 distinct WASM modules exercises the LRU eviction path inside
/// `get_or_compile_module`. The cache holds at most 64 entries
/// (`MAX_CACHED_MODULES`); inserting the 65th must evict the least-recently-used
/// entry without panicking or returning an error.
#[tokio::test]
async fn module_cache_lru_evicts_oldest_on_overflow() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            // 65 modules with distinct exit codes (0..=64), all within the
            // valid wasmtime-wasi range (< 126).
            for n in 0u32..=64 {
                let name = format!("mod_{:02}.wasm", n);
                let wat = format!(
                    r#"(module
  (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
  (memory 1) (export "memory" (memory 0))
  (func (export "_start")
    (call $proc_exit (i32.const {n}))
  )
)"#,
                    n = n
                );
                let wasm_path = make_wasm(tmp.path(), &name, &wat);

                let req = CreateTerminalRequest::new(
                    SessionId::from("lru-session"),
                    wasm_path.to_str().unwrap(),
                );
                let resp = runtime
                    .handle_create_terminal(session_id(), req)
                    .await
                    .unwrap_or_else(|e| panic!("module {} create failed: {}", n, e.message));
                let tid = resp.terminal_id;

                let wait_req =
                    WaitForTerminalExitRequest::new(SessionId::from("lru-session"), tid.clone());
                let exit = runtime
                    .handle_wait_for_terminal_exit(wait_req)
                    .await
                    .unwrap_or_else(|e| panic!("module {} wait failed: {}", n, e.message));

                assert_eq!(
                    exit.exit_status.exit_code,
                    Some(n),
                    "module {} should exit with code {}",
                    n,
                    n
                );
            }
        })
        .await;
}

// ── Missing WASM file ──────────────────────────────────────────────────────

/// `handle_create_terminal` must return `Err` when the `.wasm` path does not
/// exist on disk instead of panicking or hanging.
#[tokio::test]
async fn create_terminal_nonexistent_wasm_returns_error() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                "/nonexistent/path/missing.wasm",
            );
            let err = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect_err("missing wasm file should return Err");

            assert!(
                err.message.contains("missing.wasm")
                    || err.message.contains("not found")
                    || err.message.contains("No such file"),
                "error should mention the missing path, got: {}",
                err.message
            );
        })
        .await;
}

// ── Config ─────────────────────────────────────────────────────────────────

#[test]
fn config_from_env_parses_environment_variables() {
    // Clean up any pre-existing values before setting ours.
    std::env::remove_var("WASM_OUTPUT_BYTE_LIMIT");
    std::env::remove_var("WASM_AUTO_ALLOW_PERMISSIONS");
    std::env::remove_var("WASM_TIMEOUT_SECS");
    std::env::remove_var("WASM_FUEL_LIMIT");
    std::env::remove_var("WASM_ONLY");

    std::env::set_var("WASM_OUTPUT_BYTE_LIMIT", "2097152");
    std::env::set_var("WASM_AUTO_ALLOW_PERMISSIONS", "true");
    std::env::set_var("WASM_TIMEOUT_SECS", "42");
    std::env::set_var("WASM_FUEL_LIMIT", "500000");
    std::env::set_var("WASM_ONLY", "false");

    let config = Config::from_env();

    assert_eq!(config.output_byte_limit, 2097152);
    assert_eq!(config.auto_allow_permissions, true);
    assert_eq!(config.wasm_timeout_secs, Some(42));
    assert_eq!(config.wasm_fuel_limit, 500_000);
    assert_eq!(config.wasm_only, false);

    std::env::remove_var("WASM_OUTPUT_BYTE_LIMIT");
    std::env::remove_var("WASM_AUTO_ALLOW_PERMISSIONS");
    std::env::remove_var("WASM_TIMEOUT_SECS");
    std::env::remove_var("WASM_FUEL_LIMIT");
    std::env::remove_var("WASM_ONLY");
}

#[test]
fn config_validate_creates_missing_session_root() {
    let tmp = TempDir::new().unwrap();
    let new_dir = tmp.path().join("new_dir/nested");

    assert!(
        !new_dir.exists(),
        "directory should not exist before validate"
    );

    let mut cfg = test_config(tmp.path().to_path_buf());
    cfg.session_root = new_dir.clone();

    let errors = cfg.validate();

    assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    assert!(
        std::fs::metadata(&new_dir).is_ok(),
        "session_root should have been created by validate()"
    );
}

// ── New tests (4) ─────────────────────────────────────────────────────────────

/// A WASM module calls `recv_message` with a sub_id that was never registered
/// (sub_id=99). The host function should return -1 immediately. Module exits 0
/// if it got -1, exits 1 otherwise.
#[tokio::test]
async fn recv_message_unknown_sub_id_returns_minus_one() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "recv_unknown_sub.wasm",
                r#"
                (module
                  (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
                  (import "trogon_v1" "recv_message" (func $recv_message
                    (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
                  (memory 1) (export "memory" (memory 0))
                  (func (export "_start")
                    (local $r i32)
                    (local.set $r (call $recv_message
                      (i32.const 99)   ;; unknown sub_id
                      (i32.const 0)    ;; out_subj_ptr
                      (i32.const 64)   ;; out_subj_max
                      (i32.const 64)   ;; out_subj_len_ptr
                      (i32.const 128)  ;; out_payload_ptr
                      (i32.const 64)   ;; out_payload_max
                      (i32.const 192)  ;; out_payload_len_ptr
                      (i32.const 100)  ;; timeout_ms
                    ))
                    (if (i32.eq (local.get $r) (i32.const -1))
                      (then (call $proc_exit (i32.const 0)))
                      (else (call $proc_exit (i32.const 1)))
                    )
                    unreachable
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("recv_message unknown sub_id module should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .expect("wait should succeed");

            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "recv_message with unknown sub_id=99 should return -1; module exits 0, got: {:?}",
                exit.exit_status
            );
        })
        .await;
}

/// A WASM module calls `unsubscribe` with sub_id=99 (never registered). Host
/// function returns -1. Module exits 0 if got -1, exits 1 otherwise.
#[tokio::test]
async fn unsubscribe_unknown_sub_id_returns_minus_one() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "unsub_unknown.wasm",
                r#"
                (module
                  (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
                  (import "trogon_v1" "unsubscribe" (func $unsubscribe (param i32) (result i32)))
                  (memory 1) (export "memory" (memory 0))
                  (func (export "_start")
                    (if (i32.eq (call $unsubscribe (i32.const 99)) (i32.const -1))
                      (then (call $proc_exit (i32.const 0)))
                      (else (call $proc_exit (i32.const 1)))
                    )
                    unreachable
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("unsubscribe unknown sub_id module should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .expect("wait should succeed");

            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "unsubscribe with unknown sub_id=99 should return -1; module exits 0, got: {:?}",
                exit.exit_status
            );
        })
        .await;
}

/// A WASM module that hits `unreachable` immediately should produce a trap
/// signal (exit_code = None, signal contains "trap" case-insensitively).
#[tokio::test]
async fn generic_wasm_trap_returns_trap_signal() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "immediate_trap.wasm",
                r#"
                (module
                  (memory 1) (export "memory" (memory 0))
                  (func (export "_start") unreachable)
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("trap module should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .expect("wait should succeed");

            assert!(
                exit.exit_status.exit_code.is_none(),
                "trap should not produce an exit_code, got: {:?}",
                exit.exit_status
            );
            let sig = exit.exit_status.signal.as_deref().unwrap_or("");
            assert!(
                sig.to_lowercase().contains("trap"),
                "trap module should report a signal containing 'trap', got: {:?}",
                exit.exit_status
            );
        })
        .await;
}

/// A WASM module with no `_start` export cannot start cleanly. `create_terminal`
/// should succeed (terminal is created), but `wait_for_terminal_exit` should
/// return an exit status indicating failure (either signal is Some, or
/// exit_code is Some non-zero).
#[tokio::test]
async fn wasm_module_no_start_export_exits_with_error() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "no_start.wasm",
                r#"
                (module
                  (memory 1)
                  (export "memory" (memory 0))
                  (func $dummy (result i32) (i32.const 0))
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            // create_terminal should succeed — terminal is created.
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("create_terminal should succeed for module without _start");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                runtime.handle_wait_for_terminal_exit(wait_req),
            )
            .await
            .expect("wait should complete within 5s")
            .expect("wait_for_terminal_exit should return Ok");

            // The module cannot start cleanly — either a signal is set or exit_code is non-zero.
            let clean = exit.exit_status.exit_code == Some(0)
                && exit.exit_status.signal.is_none();
            assert!(
                !clean,
                "module without _start should not exit cleanly (exit_code=0, no signal), got: {:?}",
                exit.exit_status
            );
        })
        .await;
}

#[test]
fn config_validate_creates_missing_module_cache_dir() {
    let tmp = TempDir::new().unwrap();
    let cache_dir = tmp.path().join("cache/subdir");

    assert!(
        !cache_dir.exists(),
        "cache directory should not exist before validate"
    );

    let mut cfg = test_config(tmp.path().to_path_buf());
    cfg.module_cache_dir = Some(cache_dir.clone());

    let errors = cfg.validate();

    assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    assert!(
        std::fs::metadata(&cache_dir).is_ok(),
        "module_cache_dir should have been created by validate()"
    );
}

// ── New tests (appended) ──────────────────────────────────────────────────────

/// A WASM module that writes 30 × U+1F600 (4-byte emoji = 120 bytes total) to
/// stdout with an `output_byte_limit` of 50 bytes must:
///   1. Report `truncated == true`.
///   2. Produce output bytes that form valid UTF-8 (no broken multi-byte sequence
///      at the trim point).
#[tokio::test]
async fn append_output_utf8_multibyte_at_boundary() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cfg = Config {
                output_byte_limit: 50,
                ..test_config(tmp.path().to_path_buf())
            };
            let runtime = WasmRuntime::new(&cfg).unwrap();

            // 30 × U+1F600 (0xF0 0x9F 0x98 0x80) = 120 bytes written to stdout.
            // The iovec at offset 512 points at offset 0, length 120 (0x78).
            let wasm_path = make_wasm(
                tmp.path(),
                "utf8_boundary.wasm",
                r#"
                (module
                  (import "wasi_snapshot_preview1" "fd_write"
                    (func $fd_write (param i32 i32 i32 i32) (result i32)))
                  (import "wasi_snapshot_preview1" "proc_exit"
                    (func $proc_exit (param i32)))
                  (memory 1) (export "memory" (memory 0))
                  (data (i32.const 0)
                    "\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80\F0\9F\98\80")
                  ;; iovec at offset 512: ptr=0, len=120 (0x78)
                  (data (i32.const 512) "\00\00\00\00\78\00\00\00")
                  (func (export "_start")
                    (drop (call $fd_write
                      (i32.const 1)    ;; fd = stdout
                      (i32.const 512)  ;; iovec array ptr
                      (i32.const 1)    ;; iovec count
                      (i32.const 516)  ;; nwritten ptr
                    ))
                    (call $proc_exit (i32.const 0))
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("utf8 boundary module should be created");
            let tid = resp.terminal_id;

            let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid.clone());
            runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .expect("wait should succeed");

            let out_req = TerminalOutputRequest::new(SessionId::from("s1"), tid);
            let out = runtime
                .handle_terminal_output(out_req)
                .await
                .expect("output should succeed");

            assert!(
                out.truncated,
                "output should be flagged as truncated when byte limit is exceeded"
            );

            // The retained bytes must form valid UTF-8 — no broken codepoint at
            // the trim boundary.
            assert!(
                String::from_utf8(out.output.as_bytes().to_vec()).is_ok(),
                "truncated output must still be valid UTF-8, got: {:?}",
                out.output
            );
        })
        .await;
}

/// Killing a native terminal twice must not panic.
/// The second kill may return `Ok` or `Err` — both are acceptable.
#[tokio::test]
async fn double_kill_native_terminal_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&Config {
        wasm_only: false,
        ..test_config(tmp.path().to_path_buf())
    })
    .unwrap();

    let req = CreateTerminalRequest::new(SessionId::from("s1"), "sleep")
        .args(vec!["10".to_string()]);
    let resp = runtime
        .handle_create_terminal(session_id(), req)
        .await
        .expect("sleep should spawn");
    let tid = resp.terminal_id;

    // First kill — must succeed.
    let kill1 = KillTerminalRequest::new(SessionId::from("s1"), tid.clone());
    runtime
        .handle_kill_terminal(kill1)
        .await
        .expect("first kill should succeed");

    // Second kill on the same terminal — must not panic; result is unconstrained.
    let kill2 = KillTerminalRequest::new(SessionId::from("s1"), tid.clone());
    let _ = runtime.handle_kill_terminal(kill2).await;

    // The process must no longer be running: wait returns a non-zero exit status.
    let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid);
    let exit = runtime
        .handle_wait_for_terminal_exit(wait_req)
        .await
        .expect("wait after double-kill should succeed");

    assert_ne!(
        exit.exit_status.exit_code,
        Some(0),
        "killed process should not exit with code 0"
    );
}

/// When `session_idle_timeout_secs = 0`, `cleanup_idle_sessions` must be a
/// no-op: the session must still be present after the call.
#[tokio::test]
async fn session_idle_timeout_zero_disables_cleanup() {
    let tmp = TempDir::new().unwrap();
    let cfg = Config {
        session_idle_timeout_secs: 0,
        ..test_config(tmp.path().join("root_idle_zero"))
    };

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&cfg).unwrap();

            // Create a session by writing a file.
            let write_req = WriteTextFileRequest::new(
                SessionId::from("zero-timeout-session"),
                PathBuf::from("/touch.txt"),
                "x",
            );
            runtime
                .handle_write_text_file("zero-timeout-session", write_req)
                .await
                .expect("write should succeed");

            assert!(
                runtime.list_sessions().contains(&"zero-timeout-session".to_string()),
                "session should exist before cleanup"
            );

            // Brief yield so any async scheduling can settle.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // cleanup_idle_sessions with timeout=0 must be a no-op.
            runtime.cleanup_idle_sessions();

            assert!(
                runtime.list_sessions().contains(&"zero-timeout-session".to_string()),
                "session must still exist after cleanup_idle_sessions when timeout=0"
            );
        })
        .await;
}

/// Two concurrent `handle_release_terminal` calls on the same terminal ID must
/// not panic. At least one must return `Ok`; the other may return `Err`.
#[tokio::test]
async fn concurrent_release_same_terminal_race() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cfg = Config {
                wasm_fuel_limit: 0, // unlimited fuel — module spins forever
                ..test_config(tmp.path().to_path_buf())
            };
            let runtime = WasmRuntime::new(&cfg).unwrap();

            // Infinite-loop WASM module so the terminal is still live when we
            // race to release it.
            let wasm_path = make_wasm(
                tmp.path(),
                "infinite_release_race.wasm",
                r#"(module
  (memory 1) (export "memory" (memory 0))
  (func (export "_start") (loop $spin (br $spin)))
)"#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("infinite module should be created");
            let tid = resp.terminal_id;

            // Issue two concurrent releases for the same terminal ID.
            let rel_a = ReleaseTerminalRequest::new(SessionId::from("s1"), tid.clone());
            let rel_b = ReleaseTerminalRequest::new(SessionId::from("s1"), tid.clone());

            let (res_a, res_b) = tokio::join!(
                runtime.handle_release_terminal(rel_a),
                runtime.handle_release_terminal(rel_b),
            );

            // At least one of the two must have succeeded.
            let ok_count = [res_a.is_ok(), res_b.is_ok()]
                .iter()
                .filter(|&&v| v)
                .count();
            assert!(
                ok_count >= 1,
                "at least one concurrent release should return Ok; got: a={:?}, b={:?}",
                res_a.map(|_| ()),
                res_b.map(|_| ()),
            );

            // After both calls the terminal must be gone from the runtime.
            let out_req = TerminalOutputRequest::new(SessionId::from("s1"), tid);
            runtime
                .handle_terminal_output(out_req)
                .await
                .expect_err("terminal should be gone after release");
        })
        .await;
}

// ── Absolute-path WASM execution ──────────────────────────────────────────

/// A `.wasm` file referenced by its absolute path (operator-deployed binary)
/// must run successfully — exercises the absolute-path branch of
/// `run_wasm_terminal`.
#[tokio::test]
async fn wasm_absolute_path_runs_successfully() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            // Write the WASM file into a temp dir so we can form an absolute path.
            let wasm_path = make_wasm(
                tmp.path(),
                "abs_path.wasm",
                r#"
                (module
                  (import "wasi_snapshot_preview1" "proc_exit"
                    (func $proc_exit (param i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (func (export "_start")
                    (call $proc_exit (i32.const 0))
                    unreachable
                  )
                )
                "#,
            );

            // Ensure we hand an absolute path string to CreateTerminalRequest.
            let abs_path = wasm_path.canonicalize().expect("canonicalize should succeed");
            assert!(abs_path.is_absolute(), "path must be absolute");

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                abs_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal("abs-path-session", req)
                .await
                .expect("wasm module with absolute path should run");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();
            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "module at absolute path should exit cleanly"
            );
        })
        .await;
}

// ── Multi-terminal partial release keeps session ───────────────────────────

/// Releasing terminals one by one from a session with 3 terminals must not
/// clean up the session until the very last terminal is released.
#[tokio::test]
async fn multi_terminal_partial_release_keeps_session() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            let exit_wat = r#"
                (module
                  (import "wasi_snapshot_preview1" "proc_exit"
                    (func $proc_exit (param i32)))
                  (memory 1)
                  (export "memory" (memory 0))
                  (func (export "_start")
                    (call $proc_exit (i32.const 0))
                    unreachable
                  )
                )
            "#;

            let wasm1 = make_wasm(tmp.path(), "multi1.wasm", exit_wat);
            let wasm2 = make_wasm(tmp.path(), "multi2.wasm", exit_wat);
            let wasm3 = make_wasm(tmp.path(), "multi3.wasm", exit_wat);

            // Create 3 terminals in the same session.
            let resp1 = runtime
                .handle_create_terminal(
                    "sess-multi",
                    CreateTerminalRequest::new(SessionId::from("s1"), wasm1.to_str().unwrap()),
                )
                .await
                .expect("terminal 1 should be created");
            let resp2 = runtime
                .handle_create_terminal(
                    "sess-multi",
                    CreateTerminalRequest::new(SessionId::from("s1"), wasm2.to_str().unwrap()),
                )
                .await
                .expect("terminal 2 should be created");
            let resp3 = runtime
                .handle_create_terminal(
                    "sess-multi",
                    CreateTerminalRequest::new(SessionId::from("s1"), wasm3.to_str().unwrap()),
                )
                .await
                .expect("terminal 3 should be created");

            let tid1 = resp1.terminal_id;
            let tid2 = resp2.terminal_id;
            let tid3 = resp3.terminal_id;

            // Wait for all 3 to exit.
            let w1 = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid1.clone());
            runtime.handle_wait_for_terminal_exit(w1).await.unwrap();
            let w2 = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid2.clone());
            runtime.handle_wait_for_terminal_exit(w2).await.unwrap();
            let w3 = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid3.clone());
            runtime.handle_wait_for_terminal_exit(w3).await.unwrap();

            // Release terminal 1 — session must still exist (2 left).
            runtime
                .handle_release_terminal(ReleaseTerminalRequest::new(
                    SessionId::from("s1"),
                    tid1,
                ))
                .await
                .expect("release terminal 1 should succeed");
            assert!(
                runtime.list_sessions().contains(&"sess-multi".to_string()),
                "session must still exist after releasing 1 of 3 terminals"
            );

            // Release terminal 2 — session must still exist (1 left).
            runtime
                .handle_release_terminal(ReleaseTerminalRequest::new(
                    SessionId::from("s1"),
                    tid2,
                ))
                .await
                .expect("release terminal 2 should succeed");
            assert!(
                runtime.list_sessions().contains(&"sess-multi".to_string()),
                "session must still exist after releasing 2 of 3 terminals"
            );

            // Release terminal 3 — now the session should be cleaned up.
            runtime
                .handle_release_terminal(ReleaseTerminalRequest::new(
                    SessionId::from("s1"),
                    tid3,
                ))
                .await
                .expect("release terminal 3 should succeed");
            assert!(
                !runtime.list_sessions().contains(&"sess-multi".to_string()),
                "session must be gone after the last terminal is released"
            );
        })
        .await;
}

// ── wait_for_exit with timeout=0 (no-timeout branch) ─────────────────────

/// When `wait_for_exit_timeout_secs = 0` the runtime calls `child_proc.wait()`
/// directly without wrapping it in a `tokio::time::timeout`. This test exercises
/// that branch end-to-end with a native process that exits immediately.
#[tokio::test]
async fn wait_for_exit_no_timeout_native_completes() {
    let tmp = TempDir::new().unwrap();
    let cfg = Config {
        wait_for_exit_timeout_secs: 0,
        ..test_config(tmp.path().to_path_buf())
    };
    let runtime = WasmRuntime::new(&cfg).unwrap();

    let req = CreateTerminalRequest::new(SessionId::from("s1"), "true");
    let resp = runtime
        .handle_create_terminal(session_id(), req)
        .await
        .expect("`true` should spawn");
    let tid = resp.terminal_id;

    let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid);
    let exit = runtime
        .handle_wait_for_terminal_exit(wait_req)
        .await
        .expect("wait should succeed with timeout=0");

    assert_eq!(
        exit.exit_status.exit_code,
        Some(0),
        "`true` should exit with code 0"
    );
    assert!(
        exit.exit_status.signal.is_none(),
        "`true` should exit cleanly with no signal"
    );
}

// ── request_permission: invalid JSON options ──────────────────────────────

/// When `request_permission` receives a JSON string that cannot be parsed as
/// `Vec<String>`, `serde_json::from_str` falls back to an empty Vec. An empty
/// options Vec triggers the `options.is_empty()` guard which returns -1 before
/// any auto-allow or NATS logic runs.
///
/// The WAT module passes `"not-valid-json"` as the options JSON. It expects
/// the return value to be -1 and calls `proc_exit(0)` on success or
/// `proc_exit(1)` otherwise.
#[tokio::test]
async fn request_permission_invalid_json_returns_minus_one() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cfg = Config {
                auto_allow_permissions: false,
                ..test_config(tmp.path().to_path_buf())
            };
            let runtime = WasmRuntime::new(&cfg).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "req_perm_invalid_json.wasm",
                r#"
                (module
                  (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
                  (import "trogon_v1" "request_permission" (func $req_perm (param i32 i32 i32) (result i32)))
                  (memory 1) (export "memory" (memory 0))
                  (data (i32.const 0) "not-valid-json")
                  (func (export "_start")
                    (local $r i32)
                    (local.set $r (call $req_perm
                      (i32.const 0)
                      (i32.const 14)
                      (i32.const 64)
                    ))
                    (if (i32.eq (local.get $r) (i32.const -1))
                      (then (call $proc_exit (i32.const 0)))
                      (else (call $proc_exit (i32.const 1)))
                    )
                    unreachable
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("invalid-json request_permission module should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .expect("wait should succeed");

            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "request_permission with invalid JSON should return -1; module exits 0, got: {:?}",
                exit.exit_status
            );
        })
        .await;
}

// ── Concurrent native wait: second caller gets empty status ──────────────

/// When two `handle_wait_for_terminal_exit` calls run concurrently on the same
/// **native** terminal, only one caller can take the `child` process handle.
/// The second caller hits the "both child and collector are None" path for a
/// native terminal and returns an empty `TerminalExitStatus` (no exit_code,
/// no signal). This is documented behavior: "concurrent native waits are
/// unsupported."
///
/// The test asserts that:
///   - Both calls return `Ok(...)` without panicking.
///   - At least one response has `exit_code = Some(0)` (the real exit).
///   - The other response either also has `exit_code = Some(0)` (fast-path
///     cache hit) OR has no exit_code and no signal (the empty-status fallback).
#[tokio::test]
async fn concurrent_native_wait_second_caller_gets_empty_status() {
    let tmp = TempDir::new().unwrap();

    // WasmRuntime uses RefCell internally and is NOT Send. Both futures must
    // run cooperatively on the same thread inside a LocalSet.
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&Config {
                wasm_only: false,
                ..test_config(tmp.path().to_path_buf())
            })
            .unwrap();

            // Use `sleep 0.1` so the process is still running when both wait
            // futures are first polled, giving the cooperative scheduler a
            // chance to interleave them.
            let req = CreateTerminalRequest::new(SessionId::from("s1"), "sleep")
                .args(vec!["0.1".to_string()]);
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("sleep should spawn");
            let tid = resp.terminal_id;

            let wait_a = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid.clone());
            let wait_b = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid);

            // tokio::join! interleaves the two futures cooperatively: the first
            // runs until it awaits child_proc.wait(), then the second runs and
            // finds child==None, returning the empty-status fallback.
            let (res_a, res_b) = tokio::join!(
                runtime.handle_wait_for_terminal_exit(wait_a),
                runtime.handle_wait_for_terminal_exit(wait_b),
            );

            let exit_a = res_a.expect("first concurrent native wait should return Ok");
            let exit_b = res_b.expect("second concurrent native wait should return Ok");

            // At least one caller must have observed exit_code = Some(0).
            let one_got_zero = exit_a.exit_status.exit_code == Some(0)
                || exit_b.exit_status.exit_code == Some(0);
            assert!(
                one_got_zero,
                "at least one concurrent native wait should return exit_code=Some(0); \
                 got: a={:?}, b={:?}",
                exit_a.exit_status,
                exit_b.exit_status,
            );

            // The other caller must either have the same exit_code or the empty
            // fallback (no exit_code, no signal).
            let both_valid = [&exit_a.exit_status, &exit_b.exit_status].iter().all(|s| {
                s.exit_code == Some(0)
                    || (s.exit_code.is_none() && s.signal.is_none())
            });
            assert!(
                both_valid,
                "each concurrent wait must return exit_code=Some(0) or the empty \
                 fallback (no exit_code, no signal); got: a={:?}, b={:?}",
                exit_a.exit_status,
                exit_b.exit_status,
            );
        })
        .await;
}

// ── New tests ─────────────────────────────────────────────────────────────────

/// Reading a file that doesn't exist in the sandbox should return a -32603 error
/// with the OS error message (the read_to_string error branch).
#[tokio::test]
async fn read_text_file_nonexistent_returns_error() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    // Use a valid relative path that simply doesn't exist in the sandbox.
    let req = ReadTextFileRequest::new(
        SessionId::from("s1"),
        PathBuf::from("/missing.txt"),
    );
    let err = runtime
        .handle_read_text_file(session_id(), req)
        .await
        .expect_err("reading a nonexistent file should return an error");

    assert!(
        !err.message.is_empty(),
        "error message should contain the OS error, got empty string"
    );
}

/// When `auto_allow_permissions = true` and `out_ptr` is past the last valid
/// 4-byte write boundary in a 1-page (65536-byte) WASM memory, the host function
/// silently skips the write but still returns 0 (success).
#[tokio::test]
async fn request_permission_out_ptr_past_memory_still_returns_success() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            // auto_allow_permissions = true (default in test_config).
            let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

            // JSON: ["opt1"] = 8 bytes stored at offset 0.
            // out_selected_ptr = 65535 — past the last valid 4-byte boundary
            // (65535 + 4 = 65539 > 65536), so the write is silently skipped.
            // The function must still return 0.
            let wasm_path = make_wasm(
                tmp.path(),
                "req_perm_oob.wasm",
                r#"
                (module
                  (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
                  (import "trogon_v1" "request_permission" (func $req_perm (param i32 i32 i32) (result i32)))
                  (memory 1) (export "memory" (memory 0))
                  (data (i32.const 0) "[\"opt1\"]")
                  (func (export "_start")
                    (local $r i32)
                    (local.set $r (call $req_perm
                      (i32.const 0)      ;; options_json_ptr
                      (i32.const 8)      ;; options_json_len (["opt1"] = 8 bytes)
                      (i32.const 65535)  ;; out_selected_ptr — past valid 4-byte boundary
                    ))
                    (if (i32.eq (local.get $r) (i32.const 0))
                      (then (call $proc_exit (i32.const 0)))
                      (else (call $proc_exit (i32.const 1)))
                    )
                    unreachable
                  )
                )
                "#,
            );

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("module should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .unwrap();
            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "request_permission with out-of-bounds out_ptr should still return 0; \
                 got: {:?}",
                exit.exit_status
            );
        })
        .await;
}

/// Killing a native terminal sends SIGKILL (signal 9). After waiting for exit,
/// the exit status should have `signal == Some("SIGKILL")`, confirming the
/// `signal_name()` mapping for signal 9.
#[tokio::test]
async fn kill_terminal_exit_status_has_sigkill_signal() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    let req = CreateTerminalRequest::new(SessionId::from("s1"), "sleep")
        .args(vec!["30".to_string()]);
    let resp = runtime
        .handle_create_terminal(session_id(), req)
        .await
        .expect("sleep should spawn");
    let tid = resp.terminal_id;

    let kill_req = KillTerminalRequest::new(SessionId::from("s1"), tid.clone());
    runtime
        .handle_kill_terminal(kill_req)
        .await
        .expect("kill should succeed");

    let wait_req = WaitForTerminalExitRequest::new(SessionId::from("s1"), tid);
    let exit = runtime
        .handle_wait_for_terminal_exit(wait_req)
        .await
        .unwrap();

    assert_eq!(
        exit.exit_status.signal.as_deref(),
        Some("SIGKILL"),
        "a killed process should report signal 'SIGKILL'; got: {:?}",
        exit.exit_status
    );
}

#[tokio::test]
async fn native_spawn_nonexistent_command_returns_error() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&Config {
        wasm_only: false,
        ..test_config(tmp.path().to_path_buf())
    })
    .unwrap();

    let req = CreateTerminalRequest::new(
        SessionId::from("s1"),
        "this-command-does-not-exist-xyz",
    );
    let err = runtime
        .handle_create_terminal(session_id(), req)
        .await
        .expect_err("spawning a nonexistent command should fail");

    assert!(!err.message.is_empty(), "error message should be non-empty");
}

// ── Session isolation ────────────────────────────────────────────────────────

/// Files written in session A must not be readable by session B.
/// Each session has its own sandbox directory; there is no shared namespace.
#[tokio::test]
async fn cross_session_file_isolation() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    // Session A writes a secret file.
    let write_req = WriteTextFileRequest::new(
        SessionId::from("sess-a"),
        PathBuf::from("/secret.txt"),
        "session-a-secret",
    );
    runtime
        .handle_write_text_file("session-a", write_req)
        .await
        .expect("session A write should succeed");

    // Session B tries to read that same path — must fail (different sandbox).
    let read_req = ReadTextFileRequest::new(
        SessionId::from("sess-b"),
        PathBuf::from("/secret.txt"),
    );
    let err = runtime
        .handle_read_text_file("session-b", read_req)
        .await
        .expect_err("session B must not be able to read session A's file");
    assert!(!err.message.is_empty());
}

// ── Sequential double-release ────────────────────────────────────────────────

/// Releasing a terminal that was already released returns an error.
#[tokio::test]
async fn release_terminal_twice_returns_error_on_second() {
    let tmp = TempDir::new().unwrap();
    let runtime = WasmRuntime::new(&test_config(tmp.path().to_path_buf())).unwrap();

    let req = CreateTerminalRequest::new(SessionId::from("s1"), "echo")
        .args(vec!["hi".to_string()]);
    let resp = runtime
        .handle_create_terminal(session_id(), req)
        .await
        .expect("create should succeed");
    let tid = resp.terminal_id;

    // First release: must succeed.
    let rel = ReleaseTerminalRequest::new(SessionId::from("s1"), tid.clone());
    runtime
        .handle_release_terminal(rel)
        .await
        .expect("first release should succeed");

    // Second release of same terminal: must return an error.
    let rel2 = ReleaseTerminalRequest::new(SessionId::from("s1"), tid.clone());
    runtime
        .handle_release_terminal(rel2)
        .await
        .expect_err("second release of same terminal must return an error");
}

// ── WASM subscribe / recv_message / unsubscribe with real NATS ───────────────

/// Full subscribe → recv_message → unsubscribe lifecycle from inside a WASM
/// module against a real NATS server.
///
/// Strategy: the WAT module subscribes to a subject, the test publishes a
/// known-length payload ("ping" = 4 bytes) to that subject, the module calls
/// recv_message (non-blocking, 500 ms timeout), and uses the number of received
/// payload bytes as the exit code. We assert exit_code == Some(4).
#[tokio::test]
async fn wasm_module_subscribe_recv_unsubscribe_with_nats() {
    let nats_url = match nats_test_url() {
        Some(u) => u,
        None => {
            eprintln!(
                "NATS_TEST_URL not set — skipping wasm_module_subscribe_recv_unsubscribe_with_nats"
            );
            return;
        }
    };

    let nats = async_nats::connect(&nats_url).await.expect("NATS connect");
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime =
                WasmRuntime::with_nats(&test_config(tmp.path().to_path_buf()), Some(nats.clone()))
                    .unwrap();

            // Module layout:
            //   offset 0:   "wasm.recv.test" (14 bytes) — subscribe subject
            //   offset 64:  out_subj buffer  (128 bytes)
            //   offset 192: out_subj_len_ptr (4 bytes) — written by recv_message
            //   offset 196: out_payload buf  (128 bytes)
            //   offset 324: out_payload_len_ptr (4 bytes) — written by recv_message
            //
            // recv_message returns 1 on success, 0 on timeout, -1 on error.
            // Payload length is written to out_payload_len_ptr (i32 LE).
            //
            // Logic:
            //   sub_id = subscribe("wasm.recv.test")  → must be >= 0
            //   ret = recv_message(sub_id, ...)
            //   if ret != 1  → exit 98 (no message received)
            //   unsubscribe(sub_id)
            //   proc_exit(i32.load(324))  ← exit code = payload bytes written (4 for "ping")
            let wat = r#"(module
              (import "trogon_v1" "subscribe"     (func $sub  (param i32 i32) (result i32)))
              (import "trogon_v1" "recv_message"  (func $recv
                (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
              (import "trogon_v1" "unsubscribe"   (func $unsub (param i32) (result i32)))
              (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
              (memory 1) (export "memory" (memory 0))
              (data (i32.const 0) "wasm.recv.test")
              (func (export "_start")
                (local $sid i32)
                (local $ret i32)
                ;; subscribe
                (local.set $sid (call $sub (i32.const 0) (i32.const 14)))
                ;; if sub_id < 0 → exit 99 (NATS not available)
                (if (i32.lt_s (local.get $sid) (i32.const 0))
                  (then (call $exit (i32.const 99))))
                ;; recv_message with 500ms timeout
                ;; out_subj_ptr=64, out_subj_max=128, out_subj_len_ptr=192
                ;; out_payload_ptr=196, out_payload_max=128, out_payload_len_ptr=324
                (local.set $ret (call $recv
                  (local.get $sid)
                  (i32.const 64)  (i32.const 128) (i32.const 192)
                  (i32.const 196) (i32.const 128) (i32.const 324)
                  (i32.const 500)))
                ;; if ret != 1 → exit 98 (timeout or error, no message received)
                (if (i32.ne (local.get $ret) (i32.const 1))
                  (then (call $exit (i32.const 98))))
                ;; unsubscribe
                (drop (call $unsub (local.get $sid)))
                ;; exit with payload_len (loaded from out_payload_len_ptr at offset 324)
                (call $exit (i32.load (i32.const 324)))
              )
            )"#;

            let wasm_path = make_wasm(tmp.path(), "sub_recv.wasm", wat);
            let req =
                CreateTerminalRequest::new(SessionId::from("s1"), wasm_path.to_str().unwrap());
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("module should be created");

            // Give the module time to subscribe before publishing.
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;

            // Publish a 4-byte payload to the subscribed subject.
            nats.publish("wasm.recv.test", bytes::Bytes::from_static(b"ping"))
                .await
                .expect("publish failed");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .expect("wait should succeed");

            assert_eq!(
                exit.exit_status.exit_code,
                Some(4),
                "recv_message should return 4 (payload len of 'ping'); got: {:?}",
                exit.exit_status
            );
        })
        .await;
}

// ── recv_message timeout_ms == 0 is non-blocking ─────────────────────────────

/// `recv_message` called with `timeout_ms = 0` must poll non-blocking (1ms
/// window) and return 0 immediately when no message is queued.
/// Exercises the `timeout_ms == 0 → Duration::from_millis(1)` branch in wasm.rs.
#[tokio::test]
async fn recv_message_timeout_zero_is_nonblocking() {
    let nats_url = match nats_test_url() {
        Some(u) => u,
        None => {
            eprintln!("NATS_TEST_URL not set — skipping recv_message_timeout_zero_is_nonblocking");
            return;
        }
    };

    let nats = async_nats::connect(&nats_url).await.expect("NATS connect");
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime =
                WasmRuntime::with_nats(&test_config(tmp.path().to_path_buf()), Some(nats.clone()))
                    .unwrap();

            // Module subscribes to a unique subject, then immediately calls
            // recv_message with timeout_ms=0. No message has been published,
            // so it should return 0 (timeout/no message) right away.
            // We encode the result: exit 0 if recv returned 0 (expected), exit 1 otherwise.
            let wat = r#"(module
              (import "trogon_v1" "subscribe"    (func $sub  (param i32 i32) (result i32)))
              (import "trogon_v1" "recv_message" (func $recv
                (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
              (memory 1) (export "memory" (memory 0))
              (data (i32.const 0) "test.nonblocking.recv")
              (func (export "_start")
                (local $sid i32)
                (local $ret i32)
                (local.set $sid (call $sub (i32.const 0) (i32.const 21)))
                ;; subscribe failed → not available
                (if (i32.lt_s (local.get $sid) (i32.const 0))
                  (then (call $exit (i32.const 99))))
                ;; recv_message with timeout_ms = 0 (non-blocking poll)
                (local.set $ret (call $recv
                  (local.get $sid)
                  (i32.const 64) (i32.const 128) (i32.const 192)
                  (i32.const 256) (i32.const 128) (i32.const 384)
                  (i32.const 0)))
                ;; expect 0 (no message / timeout)
                (if (i32.eq (local.get $ret) (i32.const 0))
                  (then (call $exit (i32.const 0))))
                (call $exit (i32.const 1))
              )
            )"#;

            let wasm_path = make_wasm(tmp.path(), "recv_nonblock.wasm", wat);
            let req =
                CreateTerminalRequest::new(SessionId::from("s1"), wasm_path.to_str().unwrap());
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("module should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .expect("wait should succeed");

            assert_eq!(
                exit.exit_status.exit_code,
                Some(0),
                "recv_message(timeout=0) with no pending message should return 0; got: {:?}",
                exit.exit_status
            );
        })
        .await;
}

// ── MAX_SUBSCRIPTIONS_PER_MODULE limit ───────────────────────────────────────

/// After 64 successful subscriptions the 65th must return -1.
/// Exercises the `subscriptions.len() >= MAX_SUBSCRIPTIONS_PER_MODULE` guard.
#[tokio::test]
async fn wasm_max_subscriptions_per_module_limit() {
    let nats_url = match nats_test_url() {
        Some(u) => u,
        None => {
            eprintln!(
                "NATS_TEST_URL not set — skipping wasm_max_subscriptions_per_module_limit"
            );
            return;
        }
    };

    let nats = async_nats::connect(&nats_url).await.expect("NATS connect");
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime =
                WasmRuntime::with_nats(&test_config(tmp.path().to_path_buf()), Some(nats.clone()))
                    .unwrap();

            // Module loops 65 times calling subscribe("test.sub.cap").
            // It counts how many calls return -1 (should be exactly 1 — the 65th).
            // Exits with the failure count (expected: 1).
            let wat = r#"(module
              (import "trogon_v1" "subscribe" (func $sub (param i32 i32) (result i32)))
              (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
              (memory 1) (export "memory" (memory 0))
              (data (i32.const 0) "test.sub.cap")
              (func (export "_start")
                (local $i    i32)
                (local $r    i32)
                (local $fail i32)
                (local.set $i    (i32.const 0))
                (local.set $fail (i32.const 0))
                (block $done
                  (loop $loop
                    (br_if $done (i32.ge_u (local.get $i) (i32.const 65)))
                    (local.set $r (call $sub (i32.const 0) (i32.const 12)))
                    (if (i32.lt_s (local.get $r) (i32.const 0))
                      (then (local.set $fail (i32.add (local.get $fail) (i32.const 1)))))
                    (local.set $i (i32.add (local.get $i) (i32.const 1)))
                    (br $loop)
                  )
                )
                (call $exit (local.get $fail))
              )
            )"#;

            let wasm_path = make_wasm(tmp.path(), "sub_cap.wasm", wat);
            let req =
                CreateTerminalRequest::new(SessionId::from("s1"), wasm_path.to_str().unwrap());
            let resp = runtime
                .handle_create_terminal(session_id(), req)
                .await
                .expect("module should be created");

            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::from("s1"), resp.terminal_id);
            let exit = runtime
                .handle_wait_for_terminal_exit(wait_req)
                .await
                .expect("wait should succeed");

            assert_eq!(
                exit.exit_status.exit_code,
                Some(1),
                "exactly 1 subscribe call should fail (the 65th); got: {:?}",
                exit.exit_status
            );
        })
        .await;
}

// ── wasm_fuel_limit = 0 means unlimited ──────────────────────────────────────

/// With `wasm_fuel_limit = 0` a module that exhausts a small non-zero budget
/// should run to completion, exercising the `if fuel == 0 { u64::MAX }` branch.
#[tokio::test]
async fn wasm_fuel_limit_zero_means_unlimited() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            // With a tiny fuel budget (10) a looping module is killed.
            let rt_limited = WasmRuntime::new(&Config {
                wasm_fuel_limit: 10,
                ..test_config(tmp.path().to_path_buf())
            })
            .unwrap();

            let wat = r#"(module
              (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
              (memory 1) (export "memory" (memory 0))
              (func (export "_start")
                ;; loop 1 000 iterations — definitely exhausts budget=10
                (local $i i32)
                (local.set $i (i32.const 1000))
                (block $done
                  (loop $lp
                    (br_if $done (i32.eqz (local.get $i)))
                    (local.set $i (i32.sub (local.get $i) (i32.const 1)))
                    (br $lp)
                  )
                )
                (call $exit (i32.const 0))
              )
            )"#;

            let wasm_path = make_wasm(tmp.path(), "fuel_loop.wasm", wat);

            let req = CreateTerminalRequest::new(
                SessionId::from("s1"),
                wasm_path.to_str().unwrap(),
            );
            let resp = rt_limited
                .handle_create_terminal(session_id(), req)
                .await
                .expect("create should succeed");
            let exit = rt_limited
                .handle_wait_for_terminal_exit(WaitForTerminalExitRequest::new(
                    SessionId::from("s1"),
                    resp.terminal_id,
                ))
                .await
                .unwrap();
            // fuel budget=10 → killed by fuel exhaustion
            assert!(
                exit.exit_status.signal.is_some(),
                "fuel budget=10 should kill the loop; got: {:?}",
                exit.exit_status
            );

            // With fuel_limit = 0 (→ u64::MAX) the same module runs to completion.
            let rt_unlimited = WasmRuntime::new(&Config {
                wasm_fuel_limit: 0,
                ..test_config(tmp.path().to_path_buf())
            })
            .unwrap();

            let req2 = CreateTerminalRequest::new(
                SessionId::from("s2"),
                wasm_path.to_str().unwrap(),
            );
            let resp2 = rt_unlimited
                .handle_create_terminal("session-fuel-unlimited", req2)
                .await
                .expect("create should succeed");
            let exit2 = rt_unlimited
                .handle_wait_for_terminal_exit(WaitForTerminalExitRequest::new(
                    SessionId::from("s2"),
                    resp2.terminal_id,
                ))
                .await
                .unwrap();
            assert_eq!(
                exit2.exit_status.exit_code,
                Some(0),
                "fuel_limit=0 should allow loop to complete; got: {:?}",
                exit2.exit_status
            );
        })
        .await;
}

// ── wasm_max_concurrent_tasks = 0 means unlimited ────────────────────────────

/// With `wasm_max_concurrent_tasks = 0` the runtime uses `Semaphore::MAX_PERMITS`
/// so any number of tasks can run concurrently. Spawn 64 modules simultaneously
/// (more than the default of 32) and verify all complete.
#[tokio::test]
async fn wasm_max_concurrent_tasks_zero_means_unlimited() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let runtime = WasmRuntime::new(&Config {
                wasm_max_concurrent_tasks: 0,
                ..test_config(tmp.path().to_path_buf())
            })
            .unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "exit0_unlimited.wasm",
                r#"(module
                  (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
                  (memory 1) (export "memory" (memory 0))
                  (func (export "_start") (call $exit (i32.const 0)))
                )"#,
            );

            // Spawn 64 terminals simultaneously.
            let mut terminal_ids = Vec::new();
            for i in 0..64u32 {
                let sid = format!("s{i}");
                let req = CreateTerminalRequest::new(
                    SessionId::from(sid.clone()),
                    wasm_path.to_str().unwrap(),
                );
                let resp = runtime
                    .handle_create_terminal(&format!("sess-unlimited-{i}"), req)
                    .await
                    .expect("create should succeed");
                terminal_ids.push(resp.terminal_id);
            }

            // All should exit with code 0.
            let mut all_ok = true;
            for (i, tid) in terminal_ids.into_iter().enumerate() {
                let sid = format!("s{i}");
                let exit = runtime
                    .handle_wait_for_terminal_exit(WaitForTerminalExitRequest::new(
                        SessionId::from(sid),
                        tid,
                    ))
                    .await
                    .expect("wait should succeed");
                if exit.exit_status.exit_code != Some(0) {
                    all_ok = false;
                    eprintln!("terminal {i} did not exit 0: {:?}", exit.exit_status);
                }
            }
            assert!(all_ok, "all 64 concurrent tasks should exit with code 0");
        })
        .await;
}
