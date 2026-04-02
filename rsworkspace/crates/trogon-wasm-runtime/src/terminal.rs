use agent_client_protocol::{TerminalExitStatus, TerminalOutputResponse};
use std::sync::{Arc, Mutex};
use tracing::warn;

/// Distinguishes the two kinds of terminal the runtime can host.
///
/// Each variant carries only the state relevant to that kind, eliminating
/// the `Option<>` fields that were required when both lived in one struct.
pub(crate) enum TerminalKind {
    /// A native OS process spawned via `tokio::process::Command`.
    Native {
        /// The child process handle. Taken out when `wait_with_output` is called.
        child: Option<tokio::process::Child>,
        /// Stdin pipe. `None` after `close_stdin()` or if piping failed.
        stdin: Option<tokio::process::ChildStdin>,
    },
    /// A WASM module running as a background `spawn_local` task.
    Wasm {
        /// Shared cell written by the background task when it exits.
        /// `None` while the module is still running.
        exit_arc: Arc<Mutex<Option<TerminalExitStatus>>>,
    },
}

/// A running process (native or WASM) managed by the runtime.
///
/// Spawned by `create_terminal` and tracked by `terminal_id`.
/// Output (stdout + stderr interleaved) is buffered in `output_buf`.
/// The buffer is capped at `output_byte_limit`; oldest bytes are dropped.
pub struct WasmTerminal {
    pub(crate) kind: TerminalKind,
    pub(crate) output_buf: Arc<Mutex<Vec<u8>>>,
    pub(crate) output_byte_limit: usize,
    /// Background task that drains stdout/stderr (native) or runs the WASM module.
    pub(crate) output_collector: Option<tokio::task::JoinHandle<()>>,
    /// Cached exit status — set after `handle_wait_for_terminal_exit` completes.
    pub(crate) exit_status: Option<TerminalExitStatus>,
}

impl WasmTerminal {
    /// Collects the current output and exit status without waiting.
    ///
    /// For WASM terminals, peeks at the shared exit arc so that `terminal_output`
    /// can report completion even before `wait_for_terminal_exit` is called.
    pub fn snapshot(&self) -> TerminalOutputResponse {
        let buf = self.output_buf.lock().unwrap_or_else(|e| e.into_inner());
        let output = String::from_utf8_lossy(&buf).into_owned();
        let truncated = buf.len() >= self.output_byte_limit;
        let exit_status = self.exit_status.clone().or_else(|| {
            if let TerminalKind::Wasm { ref exit_arc } = self.kind {
                exit_arc.lock().ok().and_then(|g| g.clone())
            } else {
                None
            }
        });
        TerminalOutputResponse::new(output, truncated).exit_status(exit_status)
    }

    /// Appends bytes to the output buffer, dropping the oldest bytes if the
    /// limit would be exceeded. Preserves UTF-8 char boundaries when trimming.
    pub fn append_output(buf: &Arc<Mutex<Vec<u8>>>, limit: usize, data: &[u8]) {
        let mut guard = buf.lock().unwrap_or_else(|e| e.into_inner());
        guard.extend_from_slice(data);
        if guard.len() > limit {
            let excess = guard.len() - limit;
            let mut trim_at = excess;
            while trim_at < guard.len() && (guard[trim_at] & 0xC0) == 0x80 {
                trim_at += 1;
            }
            guard.drain(..trim_at);
        }
    }

    /// Writes bytes to the stdin pipe of a native process.
    /// Returns `true` on success, `false` for WASM terminals (no stdin) or on error.
    pub async fn write_stdin(&mut self, data: &[u8]) -> bool {
        use tokio::io::AsyncWriteExt;
        if let TerminalKind::Native { ref mut stdin, .. } = self.kind {
            if let Some(ref mut s) = stdin {
                return s.write_all(data).await.is_ok();
            }
        }
        false
    }

    /// Closes the stdin pipe of a native process, sending EOF.
    /// No-op for WASM terminals.
    pub fn close_stdin(&mut self) {
        if let TerminalKind::Native { ref mut stdin, .. } = self.kind {
            stdin.take();
        }
    }

    /// Kills the underlying native process. No-op for WASM terminals.
    /// Returns `true` if a kill signal was sent.
    pub async fn kill(&mut self) -> bool {
        if let TerminalKind::Native { ref mut child, .. } = self.kind {
            if let Some(c) = child {
                if let Err(e) = c.kill().await {
                    warn!(error = %e, "Failed to kill terminal process");
                    return false;
                }
                return true;
            }
        }
        false
    }
}

pub fn exit_status_from_std(status: &std::process::ExitStatus) -> TerminalExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return TerminalExitStatus::new().signal(Some(signal_name(sig).to_string()));
        }
    }
    TerminalExitStatus::new().exit_code(status.code().map(|c| c as u32))
}

#[cfg(unix)]
fn signal_name(sig: i32) -> &'static str {
    match sig {
        1 => "SIGHUP",
        2 => "SIGINT",
        9 => "SIGKILL",
        15 => "SIGTERM",
        _ => "UNKNOWN",
    }
}
