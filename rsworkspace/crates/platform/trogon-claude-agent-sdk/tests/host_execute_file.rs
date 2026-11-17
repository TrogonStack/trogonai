#![cfg(all(unix, feature = "test-support"))]

use std::{error::Error, ffi::OsStr, os::unix::net::UnixListener, time::Duration};
use trogon_claude_agent_sdk::adapters::{ExecuteFile, ExecuteFileRequest, ExecuteFileStatus, NativeHost};

#[test]
fn native_host_times_out_a_running_executable() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("held-open.sock");
    // Keeping the listener open leaves the relay waiting for frames until the host terminates it.
    let _listener = UnixListener::bind(&socket)?;
    let timeout = Duration::from_millis(20);
    let outcome = NativeHost.execute_file(&ExecuteFileRequest {
        program: OsStr::new(env!("CARGO_BIN_EXE_claude-sdk-relay")),
        args: &[socket.as_os_str()],
        timeout,
        max_buffer: None,
    });

    assert_eq!(outcome.status, ExecuteFileStatus::Timeout);
    assert_eq!(outcome.exit_code, None);
    assert_eq!(outcome.errno, None);
    assert_eq!(outcome.signal.as_deref(), Some("SIGTERM"));
    assert!(outcome.stdout.is_empty());
    assert!(outcome.stderr.is_empty());
    assert!(outcome.duration_ms >= timeout.as_millis());
    Ok(())
}
