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
}

#[tokio::test]
async fn wasm_module_exit_code() {
    let tmp = TempDir::new().unwrap();
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
}

#[tokio::test]
async fn wasm_module_stderr_captured() {
    let tmp = TempDir::new().unwrap();
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
