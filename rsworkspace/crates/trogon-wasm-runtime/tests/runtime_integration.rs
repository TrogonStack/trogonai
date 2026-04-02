use agent_client_protocol::{
    CreateTerminalRequest, KillTerminalRequest, ReadTextFileRequest, ReleaseTerminalRequest,
    RequestPermissionRequest, SessionId, TerminalId, TerminalOutputRequest,
    WaitForTerminalExitRequest, WriteTextFileRequest,
};
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
        session_root: tmp.path().to_path_buf(),
        output_byte_limit: 50,
        auto_allow_permissions: true,
        wasm_timeout_secs: None,
        wasm_only: false,
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
                session_root: tmp.path().to_path_buf(),
                output_byte_limit: 1024 * 1024,
                auto_allow_permissions: true,
                wasm_timeout_secs: Some(30), // generous timeout — should not fire
                wasm_only: false,
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
    let cfg = Config {
        session_root: tmp.path().to_path_buf(),
        output_byte_limit: 1024 * 1024,
        auto_allow_permissions: true,
        wasm_timeout_secs: None,
        wasm_only: true,
    };
    let runtime = WasmRuntime::new(&cfg).unwrap();

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
    let cfg = Config {
        session_root: tmp.path().to_path_buf(),
        output_byte_limit: 1024 * 1024,
        auto_allow_permissions: true,
        wasm_timeout_secs: None,
        wasm_only: true,
    };
    let runtime = WasmRuntime::new(&cfg).unwrap();

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
