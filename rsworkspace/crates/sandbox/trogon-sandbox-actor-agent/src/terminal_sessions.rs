//! Interactive terminals (pseudo-terminal sessions) inside the guest.
//!
//! This is the guest half of the terminal contract: it allocates a pseudo-terminal, runs the shell or command on its
//! slave side as a session leader with the terminal as controlling terminal, and turns what the master yields into
//! [`TerminalOutput`] frames with a gapless `seq`. The worker forwards those frames onward; the guest never talks to the
//! message bus.
//!
//! Backpressure is the terminal itself. Frames leave through a small bounded channel, so a reader that stops draining
//! makes the pump stop reading the master, the kernel buffer fills, and the shell blocks on write. Nothing is dropped
//! inside the guest, which is why `seq` never has a gap here; a gap a client observes was lost between the worker and
//! it.
//!
//! A session ends exactly once: the command exits (the pump drains what is left, reaps it and sends one `Exit` frame),
//! the caller closes it, or the frame receiver is dropped because the host connection went away. The last two hang the
//! process group up and kill it after a short grace, so a vanished host never leaves a shell behind.

use std::collections::{BTreeMap, HashMap};
use std::process::ExitStatus;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use trogon_sandbox_contract::{
    TerminalArgument, TerminalError, TerminalId, TerminalInput, TerminalOp, TerminalOutput, TerminalRequest,
    TerminalResponse,
};
use trogon_sandbox_protocol::{
    MAX_TERMINAL_OUTPUT_CHUNK_BYTES, MAX_TERMINALS_PER_LEASE, resolve_exec_cwd, terminal_data_frame,
    terminal_input_bytes,
};

use crate::LeaseBinding;
use crate::constants::{
    DEFAULT_SHELL, DEFAULT_TERM, DRAIN_IDLE, HANGUP_GRACE, INPUT_WRITE_TIMEOUT, NO_IDLE_LIMIT, TERMINAL_FRAME_BUFFER,
};
use crate::terminal_pty::{discard_forever, read_some, set_size, spawn_on_pty, write_all};
use crate::terminal_session::TerminalSession;
use crate::unix_signals::group_of;

fn invalid_request(reason: impl Into<String>) -> TerminalError {
    TerminalError::InvalidRequest { reason: reason.into() }
}

/// The terminals one guest holds. A guest serves one lease, so the per-lease cap is a cap on this registry.
#[derive(Default)]
pub struct TerminalSessions {
    sessions: Mutex<HashMap<TerminalId, Arc<TerminalSession>>>,
}

impl TerminalSessions {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn open_count(&self) -> usize {
        self.lock().len()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<TerminalId, Arc<TerminalSession>>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn get(&self, id: &TerminalId) -> Result<Arc<TerminalSession>, TerminalError> {
        self.lock().get(id).cloned().ok_or(TerminalError::NotFound)
    }

    /// Starts the terminal `request` describes and returns its frame stream.
    ///
    /// The caller must keep the receiver alive for as long as the terminal should live: dropping it hangs the terminal
    /// up.
    pub fn open(
        self: &Arc<Self>,
        binding: &LeaseBinding,
        request: &TerminalRequest,
    ) -> Result<mpsc::Receiver<TerminalOutput>, TerminalError> {
        let TerminalOp::Open {
            cols,
            rows,
            cmd,
            env,
            cwd,
            ..
        } = &request.op
        else {
            return Err(invalid_request("not an open request"));
        };
        check_open(env, cwd.as_deref())?;

        let mut sessions = self.lock();
        if sessions.contains_key(&request.terminal_id) {
            return Err(TerminalError::IdInUse);
        }
        if sessions.len() >= MAX_TERMINALS_PER_LEASE {
            let max = u32::try_from(MAX_TERMINALS_PER_LEASE).unwrap_or(u32::MAX);
            return Err(TerminalError::TooManyTerminals { max });
        }

        let command = terminal_command(binding, cmd, env, cwd.as_deref());
        let (child, master) = spawn_on_pty(command, program(cmd), cols.get(), rows.get())?;
        let session = Arc::new(TerminalSession {
            master,
            group: group_of(child.id()),
            input: tokio::sync::Mutex::new(()),
            stop: CancellationToken::new(),
        });
        sessions.insert(request.terminal_id.clone(), Arc::clone(&session));
        drop(sessions);

        let (frames, receiver) = mpsc::channel(TERMINAL_FRAME_BUFFER);
        tokio::spawn(Arc::clone(self).pump(request.terminal_id.clone(), session, child, frames));
        Ok(receiver)
    }

    /// Applies an input, resize or close. `Open` is served by [`Self::open`].
    pub async fn apply(&self, request: &TerminalRequest) -> TerminalResponse {
        let id = &request.terminal_id;
        let outcome = match &request.op {
            TerminalOp::Open { .. } => Err(invalid_request("open is served by the terminal stream")),
            TerminalOp::Input { data } => self.input(id, data).await,
            TerminalOp::Resize { cols, rows } => self.resize(id, cols.get(), rows.get()),
            // Closing an unknown id is `Done`, so a retried close is safe.
            TerminalOp::Close => {
                self.get(id).iter().for_each(|session| session.stop.cancel());
                Ok(())
            }
        };
        outcome.map_or_else(TerminalResponse::Failed, |()| TerminalResponse::Done)
    }

    /// Hangs up every terminal. Used when the guest is told its lease ended.
    pub fn close_all(&self) {
        self.lock().values().for_each(|session| session.stop.cancel());
    }

    async fn input(&self, id: &TerminalId, data: &TerminalInput) -> Result<(), TerminalError> {
        let bytes = terminal_input_bytes(data).map_err(|error| invalid_request(error.to_string()))?;
        let session = self.get(id)?;
        let _turn = session.input.lock().await;
        let written = tokio::time::timeout(INPUT_WRITE_TIMEOUT, write_all(&session.master, &bytes)).await;
        written.ok().and_then(Result::ok).ok_or(TerminalError::Exited)
    }

    fn resize(&self, id: &TerminalId, cols: u16, rows: u16) -> Result<(), TerminalError> {
        let session = self.get(id)?;
        // The kernel raises SIGWINCH for the foreground group on a size change.
        let resized = set_size(session.master.get_ref(), cols, rows);
        resized
            .inspect_err(|error| warn!(%error, "terminal resize failed"))
            .map_err(|_| TerminalError::Exited)
    }

    /// Copies output into frames until the command ends, the terminal is closed or the receiver goes away; then reaps
    /// the command and sends the single `Exit` frame.
    async fn pump(
        self: Arc<Self>,
        id: TerminalId,
        session: Arc<TerminalSession>,
        mut child: Child,
        frames: mpsc::Sender<TerminalOutput>,
    ) {
        let mut buf = vec![0u8; MAX_TERMINAL_OUTPUT_CHUNK_BYTES];
        let mut seq = 0u64;
        let mut status: Option<std::io::Result<ExitStatus>> = None;

        loop {
            // Once the command has exited, whatever it left in the terminal is still readable, so the read keeps going
            // until the terminal has been quiet for a moment.
            let limit = if status.is_some() { DRAIN_IDLE } else { NO_IDLE_LIMIT };
            let read = tokio::time::timeout(limit, read_some(&session.master, &mut buf));
            tokio::select! {
                () = session.stop.cancelled() => break,
                () = frames.closed() => break,
                waited = child.wait(), if status.is_none() => status = Some(waited),
                read = read => {
                    // End of input, an error once nothing holds the slave, or quiet after the exit: the command ended.
                    let Some(count) = read.ok().and_then(Result::ok).filter(|count| *count > 0) else { break };
                    let Ok(frame) = terminal_data_frame(seq, &buf[..count]) else { break };
                    tokio::select! {
                        sent = frames.send(frame) => {
                            let Ok(()) = sent else { break };
                            seq += 1;
                        }
                        () = session.stop.cancelled() => break,
                    }
                }
            }
        }

        // A command that is still running here was cut off, so it is hung up before it is reaped.
        let hang_up = status.is_none();
        let status = match status {
            Some(status) => status,
            None => reap(&mut child, &session, hang_up).await,
        };
        self.lock().remove(&id);
        // A command killed by a signal has no exit code, as the wire type says.
        let exit_code = status.ok().and_then(|status| status.code());
        let _ = frames.send(TerminalOutput::Exit { seq, exit_code }).await;
    }
}

/// Waits for the command to end, hanging it up first when asked, and kills it after a grace.
async fn reap(child: &mut Child, session: &TerminalSession, hang_up: bool) -> std::io::Result<ExitStatus> {
    if hang_up {
        signal_group(session.group, Signal::SIGHUP);
    }
    let wait = async {
        match tokio::time::timeout(HANGUP_GRACE, child.wait()).await {
            Ok(status) => status,
            Err(_) => {
                signal_group(session.group, Signal::SIGKILL);
                child.wait().await
            }
        }
    };
    tokio::select! {
        status = wait => status,
        never = discard_forever(&session.master) => match never {},
    }
}

/// Signals the command's whole process group. Best effort.
fn signal_group(group: Option<Pid>, signal: Signal) {
    group.inspect(|group| {
        let _ = killpg(*group, signal);
    });
}

fn program(cmd: &[TerminalArgument]) -> &str {
    cmd.first().map_or(DEFAULT_SHELL, TerminalArgument::as_str)
}

fn check_open(env: &BTreeMap<String, String>, cwd: Option<&str>) -> Result<(), TerminalError> {
    let invalid =
        |(key, value): &(&String, &String)| key.is_empty() || key.contains(['=', '\0']) || value.contains('\0');
    if let Some((key, _)) = env.iter().find(invalid) {
        return Err(invalid_request(format!("invalid environment variable `{key}`")));
    }
    if cwd.is_some_and(|dir| dir.is_empty() || dir.contains('\0')) {
        return Err(invalid_request("cwd must be a non-empty path without NUL"));
    }
    Ok(())
}

/// The lease's environment and working directory, plus the request's, with `TERM` defaulted.
fn terminal_command(
    binding: &LeaseBinding,
    cmd: &[TerminalArgument],
    env: &BTreeMap<String, String>,
    cwd: Option<&str>,
) -> Command {
    let mut command = Command::new(program(cmd));
    command.args(cmd.iter().skip(1).map(TerminalArgument::as_str));
    command.envs(&binding.env).envs(env);
    if !binding.env.contains_key("TERM") && !env.contains_key("TERM") {
        command.env("TERM", DEFAULT_TERM);
    }
    let dir = match &binding.working_dir {
        Some(default) => Some(resolve_exec_cwd(default, cwd)),
        None => cwd.map(str::to_string),
    };
    dir.inspect(|dir| {
        command.current_dir(dir);
    });
    command
}

#[cfg(test)]
mod tests;
