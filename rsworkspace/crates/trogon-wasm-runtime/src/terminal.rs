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
    pub(crate) output_buf: Arc<Mutex<Vec<u8>>>,
    pub(crate) output_byte_limit: usize,
    pub(crate) output_collector: Option<tokio::task::JoinHandle<()>>,
    pub(crate) exit_status: Option<TerminalExitStatus>,
}

impl WasmTerminal {
    /// Collects the current output and exit status without waiting.
    pub fn snapshot(&self) -> TerminalOutputResponse {
        let buf = self.output_buf.lock().expect("output_buf poisoned");
        let output = String::from_utf8_lossy(&buf).into_owned();
        let truncated = buf.len() >= self.output_byte_limit;
        TerminalOutputResponse::new(output, truncated)
            .exit_status(self.exit_status.clone())
    }

    /// Appends bytes to the output buffer, dropping the oldest bytes if the
    /// limit would be exceeded.
    pub fn append_output(buf: &Arc<Mutex<Vec<u8>>>, limit: usize, data: &[u8]) {
        let mut guard = buf.lock().expect("output_buf poisoned");
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

    /// Waits for the child process to exit and stores the exit status.
    pub async fn wait(&mut self) -> TerminalExitStatus {
        if let Some(ref cached) = self.exit_status {
            return cached.clone();
        }
        if let Some(child) = self.child.take() {
            let status = match child.wait_with_output().await {
                Ok(output) => exit_status_from_std(&output.status),
                Err(e) => {
                    warn!(error = %e, "Failed to wait for terminal process");
                    TerminalExitStatus::new()
                }
            };
            // Wait for the background output collector to finish draining
            // stdout/stderr before returning, so callers see complete output.
            if let Some(collector) = self.output_collector.take() {
                let _ = collector.await;
            }
            self.exit_status = Some(status.clone());
            status
        } else {
            self.exit_status
                .clone()
                .unwrap_or_else(TerminalExitStatus::new)
        }
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
