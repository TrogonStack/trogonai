use agent_client_protocol::{TerminalExitStatus, TerminalOutputResponse};
use std::sync::{Arc, Mutex};
use tokio::process::Child;
use tracing::warn;

/// A running process managed by the WASM runtime.
///
/// Spawned by `create_terminal` and tracked by `terminal_id`.
/// Output (stdout + stderr interleaved) is buffered in `output_buf`.
/// The buffer is capped at `output_byte_limit`; excess bytes are dropped
/// from the front to maintain the most recent output.
pub struct WasmTerminal {
    pub(crate) child: Option<Child>,
    /// Stdin pipe for native processes. `None` for WASM terminals.
    pub(crate) stdin: Option<tokio::process::ChildStdin>,
    pub(crate) output_buf: Arc<Mutex<Vec<u8>>>,
    pub(crate) output_byte_limit: usize,
    pub(crate) output_collector: Option<tokio::task::JoinHandle<()>>,
    pub(crate) exit_status: Option<TerminalExitStatus>,
    /// For WASM background tasks: shared exit status written by the background task.
    pub(crate) wasm_exit_status: Option<Arc<Mutex<Option<TerminalExitStatus>>>>,
}

impl WasmTerminal {
    /// Collects the current output and exit status without waiting.
    ///
    /// For WASM terminals, also peeks at the shared exit-status arc so that
    /// `terminal_output` can report completion even before `wait_for_terminal_exit`
    /// is called.
    pub fn snapshot(&self) -> TerminalOutputResponse {
        let buf = self.output_buf.lock().unwrap_or_else(|e| e.into_inner());
        let output = String::from_utf8_lossy(&buf).into_owned();
        let truncated = buf.len() >= self.output_byte_limit;
        let exit_status = self.exit_status.clone().or_else(|| {
            self.wasm_exit_status
                .as_ref()
                .and_then(|arc| arc.lock().ok())
                .and_then(|g| g.clone())
        });
        TerminalOutputResponse::new(output, truncated).exit_status(exit_status)
    }

    /// Appends bytes to the output buffer, dropping the oldest bytes if the
    /// limit would be exceeded.
    pub fn append_output(buf: &Arc<Mutex<Vec<u8>>>, limit: usize, data: &[u8]) {
        let mut guard = buf.lock().unwrap_or_else(|e| e.into_inner());
        guard.extend_from_slice(data);
        if guard.len() > limit {
            // Drop from the front, keeping the most recent `limit` bytes.
            // Ensure we don't split a UTF-8 multi-byte sequence.
            let excess = guard.len() - limit;
            let mut trim_at = excess;
            while trim_at < guard.len() && (guard[trim_at] & 0xC0) == 0x80 {
                trim_at += 1;
            }
            guard.drain(..trim_at);
        }
    }

    /// Writes bytes to the stdin pipe of the underlying native process.
    /// Returns `true` on success, `false` if there is no stdin pipe or the write fails.
    pub async fn write_stdin(&mut self, data: &[u8]) -> bool {
        use tokio::io::AsyncWriteExt;
        if let Some(ref mut stdin) = self.stdin {
            stdin.write_all(data).await.is_ok()
        } else {
            false
        }
    }

    /// Closes the stdin pipe, sending EOF to the process.
    pub fn close_stdin(&mut self) {
        self.stdin.take(); // drops the pipe, sends EOF
    }

    /// Kills the child process without releasing the terminal.
    /// Returns `true` if a kill signal was sent.
    pub async fn kill(&mut self) -> bool {
        if let Some(child) = &mut self.child {
            if let Err(e) = child.kill().await {
                warn!(error = %e, "Failed to kill terminal process");
                return false;
            }
            return true;
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
