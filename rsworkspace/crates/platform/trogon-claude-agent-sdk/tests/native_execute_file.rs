#![cfg(unix)]

use std::{error::Error, ffi::OsStr, io, path::Path, time::Duration};
use trogon_claude_agent_sdk::adapters::{ExecuteFile, ExecuteFileRequest, ExecuteFileStatus, NativeHost};

type TestResult = Result<(), Box<dyn Error>>;

fn assert_reaped(pid_file: &Path) -> TestResult {
    let pid = std::fs::read_to_string(pid_file)?.parse::<libc::pid_t>()?;
    assert!(pid > 0);
    // SAFETY: pid identifies this test's direct child, and waitpid permits a null status pointer.
    let waited = unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) };
    let error = io::Error::last_os_error();
    if waited == 0 {
        // SAFETY: waitpid confirmed this test's child is still running; kill and wait clean up before failing.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
            libc::waitpid(pid, std::ptr::null_mut(), 0);
        }
    }
    assert_eq!(waited, -1, "execute_file returned without reaping child {pid}");
    assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
    Ok(())
}

#[test]
fn timeout_kills_and_reaps_a_running_child_and_keeps_its_output() -> TestResult {
    let directory = tempfile::tempdir()?;
    let pid_file = directory.path().join("child.pid");
    let timeout = Duration::from_secs(2);
    let outcome = NativeHost.execute_file(&ExecuteFileRequest {
        program: OsStr::new("/bin/sh"),
        args: &[
            OsStr::new("-c"),
            OsStr::new(
                r#"printf '%s' "$$" > "$1"; printf 'stdout-before-timeout'; printf 'stderr-before-timeout' >&2; exec /bin/sleep 30"#,
            ),
            OsStr::new("native-execute-file"),
            pid_file.as_os_str(),
        ],
        timeout,
        max_buffer: None,
    });

    assert_reaped(&pid_file)?;
    assert_eq!(outcome.status, ExecuteFileStatus::Timeout);
    assert_eq!(outcome.stdout, "stdout-before-timeout");
    assert_eq!(outcome.stderr, "stderr-before-timeout");
    assert_eq!(outcome.exit_code, None);
    assert_eq!(outcome.errno, None);
    assert_eq!(outcome.signal.as_deref(), Some("SIGTERM"));
    assert!(outcome.duration_ms >= timeout.as_millis());
    assert!(outcome.duration_ms < 10_000);
    Ok(())
}

fn assert_buffer_overflow(script: &str, stdout: &str, stderr: &str) -> TestResult {
    let directory = tempfile::tempdir()?;
    let pid_file = directory.path().join("child.pid");
    let timeout = Duration::from_secs(10);
    let outcome = NativeHost.execute_file(&ExecuteFileRequest {
        program: OsStr::new("/bin/sh"),
        args: &[
            OsStr::new("-c"),
            OsStr::new(script),
            OsStr::new("native-execute-file"),
            pid_file.as_os_str(),
        ],
        timeout,
        max_buffer: Some(8),
    });

    assert_reaped(&pid_file)?;
    assert_eq!(outcome.status, ExecuteFileStatus::SpawnError);
    assert_eq!(outcome.errno.as_deref(), Some("ERR_CHILD_PROCESS_STDIO_MAXBUFFER"));
    assert_eq!(outcome.exit_code, None);
    assert_eq!(outcome.signal, None);
    assert_eq!(outcome.stdout, stdout);
    assert_eq!(outcome.stderr, stderr);
    assert!(outcome.duration_ms < timeout.as_millis());
    Ok(())
}

#[test]
fn stdout_overflow_kills_and_reaps_a_running_child_and_keeps_the_captured_prefix() -> TestResult {
    assert_buffer_overflow(
        r#"printf '%s' "$$" > "$1"; printf 'stdout-prefix-overflow'; exec /bin/sleep 30"#,
        "stdout-prefix-overflow",
        "",
    )
}

#[test]
fn stderr_overflow_kills_and_reaps_a_running_child_and_keeps_the_captured_prefix() -> TestResult {
    assert_buffer_overflow(
        r#"printf '%s' "$$" > "$1"; printf 'stderr-prefix-overflow' >&2; exec /bin/sleep 30"#,
        "",
        "stderr-prefix-overflow",
    )
}
