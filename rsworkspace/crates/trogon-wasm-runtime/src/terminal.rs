use crate::traits::ChildProcessHandle;
use agent_client_protocol::{TerminalExitStatus, TerminalOutputResponse};
use std::sync::{Arc, Mutex, OnceLock};
use tracing::warn;

/// Distinguishes the two kinds of terminal the runtime can host.
///
/// Each variant carries only the state relevant to that kind, eliminating
/// the `Option<>` fields that were required when both lived in one struct.
pub(crate) enum TerminalKind<H: ChildProcessHandle> {
    /// A native OS process spawned via `ProcessSpawner`.
    Native {
        /// The child process handle. Taken out when `wait_for_terminal_exit` is called.
        child: Option<H>,
        /// Stdin pipe. `None` after `close_stdin()` or if piping failed.
        stdin: Option<H::Stdin>,
    },
    /// A WASM module running as a background `spawn_local` task.
    Wasm {
        /// Shared cell written once by the background task when it exits.
        /// Empty while the module is still running; set exactly once on completion or kill.
        exit_arc: Arc<OnceLock<TerminalExitStatus>>,
    },
}

/// A running process (native or WASM) managed by the runtime.
///
/// Spawned by `create_terminal` and tracked by `terminal_id`.
/// Output (stdout + stderr interleaved) is buffered in `output_buf`.
/// The buffer is capped at `output_byte_limit`; oldest bytes are dropped.
pub(crate) struct WasmTerminal<H: ChildProcessHandle> {
    pub(crate) kind: TerminalKind<H>,
    pub(crate) output_buf: Arc<Mutex<Vec<u8>>>,
    /// Background task that drains stdout/stderr (native) or runs the WASM module.
    pub(crate) output_collector: Option<tokio::task::JoinHandle<()>>,
    /// Cached exit status — set after `handle_wait_for_terminal_exit` completes.
    pub(crate) exit_status: Option<TerminalExitStatus>,
    /// Set to `true` the first time `append_output` drops bytes due to the limit.
    /// Semantics: "has any output been lost?" — distinct from "is the buffer full?"
    /// which can be true even when no bytes were ever dropped.
    pub(crate) was_truncated: Arc<std::sync::atomic::AtomicBool>,
}

impl<H: ChildProcessHandle> WasmTerminal<H> {
    /// Collects the current output and exit status without waiting.
    ///
    /// For WASM terminals, peeks at the shared exit arc so that `terminal_output`
    /// can report completion even before `wait_for_terminal_exit` is called.
    pub fn snapshot(&self) -> TerminalOutputResponse {
        let buf = self.output_buf.lock().unwrap_or_else(|e| e.into_inner());
        let output = String::from_utf8_lossy(&buf).into_owned();
        let truncated = self
            .was_truncated
            .load(std::sync::atomic::Ordering::Relaxed);
        let exit_status = self.exit_status.clone().or_else(|| {
            if let TerminalKind::Wasm { ref exit_arc } = self.kind {
                exit_arc.get().cloned()
            } else {
                None
            }
        });
        TerminalOutputResponse::new(output, truncated).exit_status(exit_status)
    }


    /// Writes bytes to the stdin pipe of a native process.
    /// Returns `true` on success, `false` for WASM terminals (no stdin) or on error.
    pub async fn write_stdin(&mut self, data: &[u8]) -> bool {
        use tokio::io::AsyncWriteExt;
        if let TerminalKind::Native {
            stdin: Some(ref mut s),
            ..
        } = self.kind
        {
            return s.write_all(data).await.is_ok();
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

    /// Kills the underlying process or aborts the WASM background task.
    /// Returns `true` if a kill signal was sent (native) or the task was aborted (WASM).
    pub async fn kill(&mut self) -> bool {
        match self.kind {
            TerminalKind::Native { ref mut child, .. } => {
                if let Some(c) = child {
                    if let Err(e) = c.kill().await {
                        warn!(error = %e, "Failed to kill terminal process");
                        return false;
                    }
                    return true;
                }
                false
            }
            TerminalKind::Wasm { ref exit_arc } => {
                // Abort the spawn_local task that runs the module. The task's
                // JoinHandle is stored in output_collector; aborting it causes
                // wasmtime to drop the Store mid-execution, which is safe.
                if let Some(c) = self.output_collector.take() {
                    c.abort();
                    // Populate the exit arc so that any concurrent or subsequent
                    // wait_for_terminal_exit call sees a terminal state instead of
                    // spinning forever on yield_now waiting for a task that was
                    // aborted and will never write its own exit status.
                    // OnceLock::set is a no-op if the task already wrote its status
                    // (race between abort() and the task's final write).
                    let _ =
                        exit_arc.set(TerminalExitStatus::new().signal(Some("SIGKILL".to_string())));
                    true
                } else {
                    false
                }
            }
        }
    }
}

/// Appends bytes to the output buffer, dropping the oldest bytes if the
/// limit would be exceeded. Preserves UTF-8 char boundaries when trimming.
/// Sets `was_truncated` to `true` the first time bytes are dropped.
pub(crate) fn append_output(
    buf: &Arc<Mutex<Vec<u8>>>,
    was_truncated: &Arc<std::sync::atomic::AtomicBool>,
    limit: usize,
    data: &[u8],
) {
    let mut guard = buf.lock().unwrap_or_else(|e| e.into_inner());
    guard.extend_from_slice(data);
    if guard.len() > limit {
        let excess = guard.len() - limit;
        let mut trim_at = excess;
        while trim_at < guard.len() && (guard[trim_at] & 0xC0) == 0x80 {
            trim_at += 1;
        }
        guard.drain(..trim_at);
        was_truncated.store(true, std::sync::atomic::Ordering::Relaxed);
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
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        16 => "SIGUSR1",
        17 => "SIGUSR2",
        _ => "UNKNOWN",
    }
}
