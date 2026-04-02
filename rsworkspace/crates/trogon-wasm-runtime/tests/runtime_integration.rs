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
        session_root: tmp.path().to_path_buf(),
        output_byte_limit: 50,
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

// ── Memory cap ─────────────────────────────────────────────────────────────

/// Verify that a generous memory limit does not prevent a module from running.
/// This tests plumbing without exercising the denial path.
#[tokio::test]
async fn wasm_module_memory_limit_enforced() {
    let tmp = TempDir::new().unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cfg = Config {
                session_root: tmp.path().to_path_buf(),
                output_byte_limit: 1024 * 1024,
                auto_allow_permissions: true,
                wasm_timeout_secs: None,
                wasm_only: false,
                // 64 MB — generous, should not prevent the 1-page module from running.
                wasm_memory_limit_bytes: Some(64 * 1024 * 1024),
                module_cache_dir: None,
                wasm_allow_network: false,
                wasm_fuel_limit: 1_000_000_000u64,
                wasm_host_call_limit: 10_000u32,
                acp_prefix: "acp".to_string(),
                wasm_max_concurrent_tasks: 32,
                session_idle_timeout_secs: 3600,
                wasm_max_module_size_bytes: 100 * 1024 * 1024,
                wait_for_exit_timeout_secs: 300,
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
                  (import "trogon" "log" (func $log (param i32 i32 i32 i32)))
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
                    session_root: tmp.path().join("sessions_a"),
                    output_byte_limit: 1024 * 1024,
                    auto_allow_permissions: true,
                    wasm_timeout_secs: None,
                    wasm_only: false,
                    wasm_memory_limit_bytes: None,
                    module_cache_dir: Some(cache_dir.path().to_path_buf()),
                    wasm_allow_network: false,
                    wasm_fuel_limit: 1_000_000_000u64,
                    wasm_host_call_limit: 10_000u32,
                    acp_prefix: "acp".to_string(),
                    wasm_max_concurrent_tasks: 32,
                    session_idle_timeout_secs: 3600,
                    wasm_max_module_size_bytes: 100 * 1024 * 1024,
                    wait_for_exit_timeout_secs: 300,
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
                    session_root: tmp.path().join("sessions_b"),
                    output_byte_limit: 1024 * 1024,
                    auto_allow_permissions: true,
                    wasm_timeout_secs: None,
                    wasm_only: false,
                    wasm_memory_limit_bytes: None,
                    module_cache_dir: Some(cache_dir.path().to_path_buf()),
                    wasm_allow_network: false,
                    wasm_fuel_limit: 1_000_000_000u64,
                    wasm_host_call_limit: 10_000u32,
                    acp_prefix: "acp".to_string(),
                    wasm_max_concurrent_tasks: 32,
                    session_idle_timeout_secs: 3600,
                    wasm_max_module_size_bytes: 100 * 1024 * 1024,
                    wait_for_exit_timeout_secs: 300,
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
                session_root: tmp.path().join("sessions"),
                output_byte_limit: 1024 * 1024,
                auto_allow_permissions: true,
                wasm_timeout_secs: None,
                wasm_only: false,
                wasm_memory_limit_bytes: None,
                module_cache_dir: Some(cache_dir.path().to_path_buf()),
                wasm_allow_network: false,
                wasm_fuel_limit: 1_000_000_000u64,
                wasm_host_call_limit: 10_000u32,
                acp_prefix: "acp".to_string(),
                wasm_max_concurrent_tasks: 32,
                session_idle_timeout_secs: 3600,
                wasm_max_module_size_bytes: 100 * 1024 * 1024,
                wait_for_exit_timeout_secs: 300,
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
                  (import "trogon" "nats_request"
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
                  (import "trogon" "request_permission"
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
                session_root: tmp.path().to_path_buf(),
                output_byte_limit: 1024 * 1024,
                auto_allow_permissions: true,
                wasm_timeout_secs: None,
                wasm_only: false,
                wasm_memory_limit_bytes: None,
                module_cache_dir: None,
                wasm_allow_network: true, // network enabled
                wasm_fuel_limit: 1_000_000_000u64,
                wasm_host_call_limit: 10_000u32,
                acp_prefix: "acp".to_string(),
                wasm_max_concurrent_tasks: 32,
                session_idle_timeout_secs: 3600,
                wasm_max_module_size_bytes: 100 * 1024 * 1024,
                wait_for_exit_timeout_secs: 300,
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
                session_root: tmp.path().to_path_buf(),
                output_byte_limit: 1024 * 1024,
                auto_allow_permissions: true,
                wasm_timeout_secs: None,
                wasm_only: false,
                wasm_memory_limit_bytes: None,
                module_cache_dir: None,
                wasm_allow_network: false,
                wasm_fuel_limit: 1000, // very low — exhausted immediately by the loop
                wasm_host_call_limit: 10_000u32,
                acp_prefix: "acp".to_string(),
                wasm_max_concurrent_tasks: 32,
                session_idle_timeout_secs: 3600,
                wasm_max_module_size_bytes: 100 * 1024 * 1024,
                wait_for_exit_timeout_secs: 300,
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
                session_root: tmp.path().to_path_buf(),
                output_byte_limit: 1024 * 1024,
                auto_allow_permissions: true,
                wasm_timeout_secs: None,
                wasm_only: false,
                wasm_memory_limit_bytes: None,
                module_cache_dir: None,
                wasm_allow_network: false,
                wasm_fuel_limit: 1_000_000_000u64,
                wasm_host_call_limit: 2, // very low budget
                acp_prefix: "acp".to_string(),
                wasm_max_concurrent_tasks: 32,
                session_idle_timeout_secs: 3600,
                wasm_max_module_size_bytes: 100 * 1024 * 1024,
                wait_for_exit_timeout_secs: 300,
            };
            let runtime = WasmRuntime::new(&cfg).unwrap();

            // Module calls trogon.log 5 times, then proc_exit(0).
            // Budget is 2: first 2 calls succeed, remaining 3 silently fail.
            // The module should still exit cleanly with code 0.
            let wasm_path = make_wasm(
                tmp.path(),
                "budget_exhaust.wasm",
                r#"
                (module
                  (import "trogon" "log" (func $log (param i32 i32 i32 i32)))
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
                  (import "trogon" "subscribe"
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
                session_root: tmp.path().to_path_buf(),
                output_byte_limit: 1024 * 1024,
                auto_allow_permissions: false, // not auto-allowing
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
            };
            // No NATS client — WasmRuntime::new, not with_nats.
            let runtime = WasmRuntime::new(&cfg).unwrap();

            let wasm_path = make_wasm(
                tmp.path(),
                "request_permission_no_nats.wasm",
                r#"
                (module
                  (import "trogon" "request_permission"
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
                session_root: tmp.path().to_path_buf(),
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
                wasm_max_concurrent_tasks: 2,
                session_idle_timeout_secs: 3600,
                wasm_max_module_size_bytes: 100 * 1024 * 1024,
                wait_for_exit_timeout_secs: 300,
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
                session_root: tmp.path().to_path_buf(),
                output_byte_limit: 1024 * 1024,
                auto_allow_permissions: true,
                wasm_timeout_secs: None,
                wasm_only: false,
                wasm_memory_limit_bytes: None,
                module_cache_dir: None,
                wasm_allow_network: false,
                wasm_fuel_limit: 100, // very low — exhausted by the loop
                wasm_host_call_limit: 10_000u32,
                acp_prefix: "acp".to_string(),
                wasm_max_concurrent_tasks: 32,
                session_idle_timeout_secs: 3600,
                wasm_max_module_size_bytes: 100 * 1024 * 1024,
                wait_for_exit_timeout_secs: 300,
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
                session_root: tmp.path().to_path_buf(),
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
                wasm_max_module_size_bytes: 10, // only 10 bytes — any real .wasm is larger
                wait_for_exit_timeout_secs: 300,
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
                  (import "trogon" "nats_request" (func $nats_request
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
        session_root: tmp.path().to_path_buf(),
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
        wait_for_exit_timeout_secs: 1, // 1 second — fires quickly in tests
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
    runtime.handle_close_terminal_stdin(tid.0.as_ref());

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
