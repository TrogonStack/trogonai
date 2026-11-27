//! Interactive terminals for the subprocess worker.
//!
//! A terminal is a second payload launched through the lease's own isolation
//! backend from the lease's own launch spec, so it sees the same filesystem,
//! network policy, user and secrets as the lease's process. Its standard streams are
//! the slave side of a fresh pseudo-terminal; the worker holds the master,
//! writes input to it and publishes what it reads as output frames on core
//! NATS. Under cgroup limits a terminal joins
//! the lease's cgroup, so the lease and its terminals share one budget; under
//! rlimits each process carries the lease's per-process limits.

use std::collections::{BTreeMap, HashMap};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use nix::fcntl::{FcntlArg, FdFlag, OFlag, fcntl};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::io::unix::AsyncFd;
use tokio::process::Child;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use trogon_sandbox_contract::{
    LeaseId, ProviderKind, TerminalError, TerminalId, TerminalOp, TerminalOutput, TerminalRequest, TerminalResponse,
};
use trogon_sandbox_protocol::subjects::{provider_terminal, terminal_output, worker_lease_activity};
use trogon_sandbox_protocol::{
    ActivitySignal, MAX_TERMINAL_OUTPUT_CHUNK_BYTES, MAX_TERMINALS_PER_LEASE, TerminalReplayRing, terminal_data_frame,
    terminal_input_bytes, terminal_replay_bytes_from_env,
};
use trogon_std::env::ReadEnv;

use crate::constants::{DEFAULT_SHELL, DEFAULT_TERM, HANGUP_GRACE, INPUT_WRITE_TIMEOUT};
use crate::isolation::{IsolationBackend, LaunchedProcess, TerminalStdio};
use crate::launch::LaunchSpec;
use crate::placeholders::{PlaceholderHold, PlaceholderLedger};

/// Terminal requests the broker routes to subprocess workers. Every worker
/// receives each one; only the worker holding the lease replies.
pub fn terminal_subject() -> String {
    provider_terminal(ProviderKind::Subprocess, &LeaseId::new(">"))
}

/// Locks a mutex whose holders never leave it half-updated, so a poisoned
/// lock is still usable.
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where output frames go. The worker publishes on core NATS; tests collect.
#[async_trait]
pub trait FrameSink: Send + Sync {
    async fn publish(&self, subject: String, payload: Bytes);
}

#[async_trait]
impl FrameSink for async_nats::Client {
    async fn publish(&self, subject: String, payload: Bytes) {
        if let Err(e) = async_nats::Client::publish(self, subject, payload).await {
            warn!(error = %e, "terminal frame publish failed");
        }
    }
}

/// What a terminal of one lease is launched from.
struct LeaseTarget {
    spec: LaunchSpec,
    backend: Arc<dyn IsolationBackend>,
    preview: Option<crate::port_forward::LeasePreview>,
}

struct Terminal {
    lease_id: LeaseId,
    /// The command's process group: it is a session leader, so its pid.
    pgid: Pid,
    master: Arc<AsyncFd<OwnedFd>>,
    input: tokio::sync::Mutex<()>,
    /// Next output `seq`, or `None` once the `Exit` frame went out. Held
    /// across each publish so frames leave in order and none follow `Exit`.
    seq: tokio::sync::Mutex<Option<u64>>,
    /// The latest output, for a reader that reattaches. Only touched with
    /// `seq` held, so it always matches what was published.
    replay: Mutex<TerminalReplayRing>,
    stop: CancellationToken,
    /// Paces the activity signal output sends, so a terminal that only
    /// prints still counts toward the idle timeout.
    activity: ActivitySignal,
    preview: Mutex<Option<crate::port_forward::PreviewMembership>>,
}

/// The terminals one subprocess worker holds, across all its leases.
pub struct SubprocessTerminals {
    sink: Arc<dyn FrameSink>,
    leases: Mutex<HashMap<LeaseId, Arc<LeaseTarget>>>,
    terminals: Mutex<HashMap<TerminalId, Arc<Terminal>>>,
    /// Serializes `open` so the per-lease cap and the id check cannot race.
    open_gate: tokio::sync::Mutex<()>,
    /// Size of each new terminal's replay ring, in bytes.
    replay_bytes: usize,
}

impl SubprocessTerminals {
    /// Terminals whose replay ring is sized by
    /// [`trogon_sandbox_protocol::TERMINAL_REPLAY_BYTES_ENV`] in `env`.
    pub fn new(sink: Arc<dyn FrameSink>, env: &impl ReadEnv) -> Arc<Self> {
        Self::with_replay_bytes(sink, terminal_replay_bytes_from_env(env))
    }

    /// Terminals with a replay ring of `replay_bytes` each (zero: no replay).
    pub fn with_replay_bytes(sink: Arc<dyn FrameSink>, replay_bytes: usize) -> Arc<Self> {
        Arc::new(Self {
            replay_bytes,
            sink,
            leases: Mutex::new(HashMap::new()),
            terminals: Mutex::new(HashMap::new()),
            open_gate: tokio::sync::Mutex::new(()),
        })
    }

    /// Makes `lease_id` reachable for terminals, launched from `spec`.
    pub fn register_lease(&self, lease_id: &LeaseId, spec: LaunchSpec, backend: Arc<dyn IsolationBackend>) {
        self.register_lease_with_preview(lease_id, spec, backend, None);
    }

    pub(crate) fn register_lease_with_preview(
        &self,
        lease_id: &LeaseId,
        spec: LaunchSpec,
        backend: Arc<dyn IsolationBackend>,
        preview: Option<crate::port_forward::LeasePreview>,
    ) {
        let replaced = locked(&self.leases).insert(lease_id.clone(), Arc::new(LeaseTarget { spec, backend, preview }));
        if let Some(replaced) = replaced
            && let Some(preview) = &replaced.preview
        {
            preview.revoke();
        }
    }

    /// Whether this worker holds `lease_id`.
    pub fn holds(&self, lease_id: &LeaseId) -> bool {
        locked(&self.leases).contains_key(lease_id)
    }

    /// Ends every terminal of a lease that ended and stops accepting new ones.
    pub async fn forget_lease(&self, lease_id: &LeaseId) {
        let (target, terminals) = {
            let mut leases = locked(&self.leases);
            let target = leases.remove(lease_id);
            let terminals: Vec<(TerminalId, Arc<Terminal>)> = locked(&self.terminals)
                .iter()
                .filter(|(_, t)| t.lease_id == *lease_id)
                .map(|(id, terminal)| (id.clone(), terminal.clone()))
                .collect();
            (target, terminals)
        };
        if let Some(preview) = target.as_ref().and_then(|target| target.preview.as_ref()) {
            preview.revoke();
        }
        for (id, terminal) in terminals {
            self.finish(&id, &terminal, None).await;
        }
    }

    pub fn open_count(&self, lease_id: &LeaseId) -> usize {
        locked(&self.terminals)
            .values()
            .filter(|t| t.lease_id == *lease_id)
            .count()
    }

    /// Answers one terminal request, or `None` when this worker does not hold
    /// the lease (another worker answers).
    pub async fn handle(self: &Arc<Self>, body: &[u8]) -> Option<TerminalResponse> {
        let req = match serde_json::from_slice::<TerminalRequest>(body) {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "invalid terminal request");
                return None;
            }
        };
        if !self.holds(&req.lease_id) {
            return None;
        }
        let lease = &req.lease_id;
        let id = &req.terminal_id;
        Some(match &req.op {
            TerminalOp::Open {
                cols,
                rows,
                cmd,
                env,
                cwd,
                resume_from,
            } => {
                let size = (cols.get(), rows.get());
                let command: Vec<String> = cmd.iter().map(|arg| arg.as_str().to_owned()).collect();
                let opened = (command.as_slice(), env, cwd.as_deref());
                self.open(&req.lease_id, id, size, opened, *resume_from).await
            }
            TerminalOp::Input { data } => {
                let bytes = match terminal_input_bytes(data) {
                    Ok(b) => b,
                    Err(e) => {
                        return Some(TerminalResponse::Failed(TerminalError::InvalidRequest {
                            reason: e.to_string(),
                        }));
                    }
                };
                match self.owned(lease, id) {
                    Err(e) => TerminalResponse::Failed(e),
                    Ok(t) => {
                        let _writer = t.input.lock().await;
                        match tokio::time::timeout(INPUT_WRITE_TIMEOUT, write_all(&t.master, &bytes)).await {
                            Ok(Ok(())) => TerminalResponse::Done,
                            // The command is gone or not reading: only close helps.
                            _ => TerminalResponse::Failed(TerminalError::Exited),
                        }
                    }
                }
            }
            TerminalOp::Resize { cols, rows } => match self.owned(lease, id) {
                Err(e) => TerminalResponse::Failed(e),
                Ok(t) => match set_size(t.master.get_ref(), cols.get(), rows.get()) {
                    Ok(()) => TerminalResponse::Done,
                    Err(e) => {
                        warn!(error = %e, "terminal resize failed");
                        TerminalResponse::Failed(TerminalError::Exited)
                    }
                },
            },
            TerminalOp::Close => match self.owned(lease, id) {
                // A retried close, or a close of an unknown id, is not an error.
                Err(TerminalError::NotFound) => TerminalResponse::Done,
                Err(e) => TerminalResponse::Failed(e),
                Ok(terminal) => {
                    self.finish(id, &terminal, None).await;
                    TerminalResponse::Done
                }
            },
        })
    }

    /// Answers terminal requests on [`terminal_subject`] until `cancel`.
    pub async fn serve(self: Arc<Self>, client: async_nats::Client, cancel: CancellationToken) -> anyhow::Result<()> {
        let mut requests = trogon_sandbox_protocol::subscribe_verified(&client, terminal_subject()).await?;
        loop {
            let msg = tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                msg = requests.next() => match msg {
                    Some(m) => m,
                    None => return Ok(()),
                },
            };
            let Some(reply) = msg.reply.clone() else {
                continue;
            };
            let terminals = Arc::clone(&self);
            let client = client.clone();
            tokio::spawn(async move {
                let Some(response) = terminals.handle(&msg.payload).await else {
                    return;
                };
                match serde_json::to_vec(&response) {
                    Ok(body) => {
                        if let Err(e) = client.publish(reply, Bytes::from(body)).await {
                            warn!(error = %e, "terminal reply publish failed");
                        }
                    }
                    Err(e) => warn!(error = %e, "terminal reply encode failed"),
                }
            });
        }
    }

    async fn open(
        self: &Arc<Self>,
        lease_id: &LeaseId,
        id: &TerminalId,
        size: (u16, u16),
        (cmd, env, cwd): (&[String], &BTreeMap<String, String>, Option<&str>),
        resume_from: Option<u64>,
    ) -> TerminalResponse {
        let _gate = self.open_gate.lock().await;
        let existing = locked(&self.terminals).get(id).cloned();
        if let Some(existing) = existing {
            if existing.lease_id != *lease_id {
                return TerminalResponse::Failed(TerminalError::IdInUse);
            }
            self.reattach(id, &existing, resume_from).await;
            return TerminalResponse::Opened {
                terminal_id: id.clone(),
            };
        }
        {
            let map = locked(&self.terminals);
            if map.values().filter(|t| t.lease_id == *lease_id).count() >= MAX_TERMINALS_PER_LEASE {
                return TerminalResponse::Failed(TerminalError::TooManyTerminals {
                    max: MAX_TERMINALS_PER_LEASE as u32,
                });
            }
        }
        if let Err(reason) = check_open(env, cwd) {
            return TerminalResponse::Failed(TerminalError::InvalidRequest { reason });
        }
        let Some(target) = locked(&self.leases).get(lease_id).cloned() else {
            return TerminalResponse::Failed(TerminalError::SpawnFailed {
                reason: "the worker holds no process for this lease".to_string(),
            });
        };
        let spawn_failed = |reason: String| TerminalResponse::Failed(TerminalError::SpawnFailed { reason });

        let pty = match nix::pty::openpty(Some(&winsize(size.0, size.1)), None) {
            Ok(p) => p,
            Err(e) => return spawn_failed(format!("allocating a pseudo-terminal: {e}")),
        };
        // openpty leaves both ends inheritable. A command spawned at the same
        // time for another lease or terminal would keep the slave open, and
        // this terminal would never see its command end.
        if let Err(e) = close_on_exec(&pty.master).and_then(|()| close_on_exec(&pty.slave)) {
            return spawn_failed(format!("allocating a pseudo-terminal: {e}"));
        }
        let spec = terminal_spec(&target.spec, cmd, env, cwd);
        let hold = PlaceholderLedger::shared().begin();
        let tty = TerminalStdio::new(pty.slave);
        let launched = target.backend.launch_terminal(&spec, &tty).await;
        // The parent's copy of the slave must close, or the master never
        // sees the command's end.
        drop(tty);
        let launched = match launched {
            Ok(l) => l,
            Err(e) => return spawn_failed(format!("{e:#}")),
        };
        hold.record(&launched.placeholders);
        #[cfg(target_os = "linux")]
        let preview = match &target.preview {
            Some(preview) => match preview.register_process(launched.pid, launched.child.clone()).await {
                Ok(membership) => membership,
                Err(error) => {
                    let pgid = Pid::from_raw(i32::try_from(launched.pid).unwrap_or(0));
                    tokio::spawn(reap(launched, pgid, hold, true));
                    return spawn_failed(format!("retaining terminal preview authority: {error}"));
                }
            },
            None => None,
        };
        #[cfg(not(target_os = "linux"))]
        let preview = None;
        let master = match nonblocking(pty.master).and_then(watch) {
            Ok(m) => Arc::new(m),
            Err(e) => {
                let pgid = Pid::from_raw(i32::try_from(launched.pid).unwrap_or(0));
                tokio::spawn(reap(launched, pgid, hold, true));
                return spawn_failed(format!("watching the pseudo-terminal: {e}"));
            }
        };
        let terminal = Arc::new(Terminal {
            lease_id: lease_id.clone(),
            pgid: Pid::from_raw(i32::try_from(launched.pid).unwrap_or(0)),
            master,
            input: tokio::sync::Mutex::new(()),
            seq: tokio::sync::Mutex::new(Some(0)),
            replay: Mutex::new(TerminalReplayRing::new(self.replay_bytes)),
            stop: CancellationToken::new(),
            activity: ActivitySignal::default(),
            preview: Mutex::new(preview),
        });
        let admitted = {
            let leases = locked(&self.leases);
            if leases
                .get(lease_id)
                .is_some_and(|current| Arc::ptr_eq(current, &target))
                && target.preview.as_ref().is_none_or(|preview| !preview.is_revoked())
            {
                locked(&self.terminals).insert(id.clone(), Arc::clone(&terminal));
                true
            } else {
                false
            }
        };
        if !admitted {
            let pgid = terminal.pgid;
            drop(terminal);
            tokio::spawn(reap(launched, pgid, hold, true));
            return spawn_failed("the lease ended while its terminal was starting".into());
        }
        tokio::spawn(Arc::clone(self).pump(id.clone(), terminal, launched, hold));
        TerminalResponse::Opened {
            terminal_id: id.clone(),
        }
    }

    /// Republishes what a reader that reattached missed (see
    /// [`TerminalReplayRing::reattach_frames`]). Holding `seq` keeps the
    /// replay ahead of any live frame.
    async fn reattach(&self, id: &TerminalId, terminal: &Terminal, resume_from: Option<u64>) {
        let seq = terminal.seq.lock().await;
        if seq.is_none() {
            return;
        }
        let frames = match locked(&terminal.replay).reattach_frames(resume_from) {
            Ok(frames) => frames,
            Err(error) => {
                warn!(%error, "terminal replay frames could not be encoded");
                return;
            }
        };
        for frame in frames {
            publish(self.sink.as_ref(), id, &frame).await;
        }
    }

    fn owned(&self, lease_id: &LeaseId, id: &TerminalId) -> Result<Arc<Terminal>, TerminalError> {
        match locked(&self.terminals).get(id) {
            None => Err(TerminalError::NotFound),
            Some(t) if t.lease_id == *lease_id => Ok(Arc::clone(t)),
            // Another lease's terminal never reads as this lease's data.
            Some(_) => Err(TerminalError::IdInUse),
        }
    }

    /// Ends `id` once: whoever removes it from the map sends the `Exit`
    /// frame. The pump then hangs the command up and reaps it.
    async fn finish(&self, id: &TerminalId, expected: &Arc<Terminal>, exit_code: Option<i32>) {
        let terminal = {
            let mut terminals = locked(&self.terminals);
            if !terminals.get(id).is_some_and(|current| Arc::ptr_eq(current, expected)) {
                return;
            }
            let Some(terminal) = terminals.remove(id) else {
                return;
            };
            terminal
        };
        locked(&terminal.preview).take();
        terminal.stop.cancel();
        let mut seq = terminal.seq.lock().await;
        if let Some(next) = seq.take() {
            let frame = TerminalOutput::Exit { seq: next, exit_code };
            publish(self.sink.as_ref(), id, &frame).await;
        }
    }

    /// Copies output to frames until the command ends or the terminal is
    /// closed, then reaps the command and releases what its launch holds.
    async fn pump(
        self: Arc<Self>,
        id: TerminalId,
        terminal: Arc<Terminal>,
        launched: LaunchedProcess,
        hold: PlaceholderHold,
    ) {
        let mut buf = vec![0u8; MAX_TERMINAL_OUTPUT_CHUNK_BYTES];
        let closed = loop {
            let n = tokio::select! {
                _ = terminal.stop.cancelled() => break true,
                n = read_some(&terminal.master, &mut buf) => n,
            };
            // EOF, or EIO once no process holds the slave: the command ended.
            let Ok(n) = n else { break false };
            if n == 0 {
                break false;
            }
            {
                let mut seq = terminal.seq.lock().await;
                let Some(next) = seq.as_mut() else { break true };
                locked(&terminal.replay).push(*next, &buf[..n]);
                match terminal_data_frame(*next, &buf[..n]) {
                    Ok(frame) => publish(self.sink.as_ref(), &id, &frame).await,
                    Err(error) => warn!(%error, "terminal output could not be encoded"),
                }
                *next += 1;
            }
            if terminal.activity.due(Instant::now()) {
                let subject = worker_lease_activity(&terminal.lease_id);
                self.sink.publish(subject, Bytes::new()).await;
            }
        };
        let code = reap(launched, terminal.pgid, hold, closed).await;
        self.finish(&id, &terminal, code).await;
    }
}

/// Waits for the command to end, hanging it up first when `hang_up`, and
/// releases its limits and placeholders. Returns its exit code.
async fn reap(launched: LaunchedProcess, pgid: Pid, hold: PlaceholderHold, hang_up: bool) -> Option<i32> {
    let LaunchedProcess {
        child,
        limits,
        monitors,
        ..
    } = launched;
    if hang_up {
        signal_group(pgid, Signal::SIGHUP);
    }
    let mut guard = child.lock().await;
    let code = match guard.as_mut() {
        Some(proc) => wait_or_kill(proc, pgid, hang_up).await,
        None => None,
    };
    drop(guard);
    limits.release().await;
    hold.release();
    drop(monitors);
    code
}

async fn wait_or_kill(proc: &mut Child, pgid: Pid, hung_up: bool) -> Option<i32> {
    let grace = if hung_up { HANGUP_GRACE } else { HANGUP_GRACE * 2 };
    let status = match tokio::time::timeout(grace, proc.wait()).await {
        Ok(status) => status,
        Err(_) => {
            signal_group(pgid, Signal::SIGKILL);
            let _ = proc.start_kill();
            proc.wait().await
        }
    };
    // A command killed by a signal has no exit code, as the wire type says.
    status.ok().and_then(|s| s.code())
}

/// Signals the command's whole process group. Best effort: a group that
/// already exited leaves nothing to signal.
fn signal_group(pgid: Pid, signal: Signal) {
    if pgid.as_raw() > 0 {
        let _ = killpg(pgid, signal);
    }
}

/// Refuses environment names and directories no process can be given.
fn check_open(env: &BTreeMap<String, String>, cwd: Option<&str>) -> Result<(), String> {
    for (key, value) in env {
        if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
            return Err(format!("invalid environment variable `{key}`"));
        }
    }
    if cwd.is_some_and(|dir| dir.is_empty() || dir.contains('\0')) {
        return Err("cwd must be a non-empty path without NUL".to_string());
    }
    Ok(())
}

/// The lease's launch spec, running `cmd` (a shell when empty) in `cwd`
/// with `env` added and `TERM` defaulted.
fn terminal_spec(lease: &LaunchSpec, cmd: &[String], env: &BTreeMap<String, String>, cwd: Option<&str>) -> LaunchSpec {
    let mut spec = lease.clone();
    let command: Vec<String> = if cmd.is_empty() {
        vec![DEFAULT_SHELL.to_string()]
    } else {
        cmd.to_vec()
    };
    match cwd {
        // The backends start in the workspace; a shell moves there first.
        Some(dir) => {
            spec.binary = PathBuf::from(DEFAULT_SHELL);
            spec.args = vec![
                "-c".to_string(),
                "cd -- \"$0\" && exec \"$@\"".to_string(),
                dir.to_string(),
            ];
            spec.args.extend(command);
        }
        None => {
            spec.binary = PathBuf::from(&command[0]);
            spec.args = command[1..].to_vec();
        }
    }
    spec.env.retain(|(key, _)| !env.contains_key(key));
    spec.env.extend(env.iter().map(|(k, v)| (k.clone(), v.clone())));
    if !spec.env.iter().any(|(key, _)| key == "TERM") {
        spec.env.push(("TERM".to_string(), DEFAULT_TERM.to_string()));
    }
    spec
}

fn winsize(cols: u16, rows: u16) -> nix::pty::Winsize {
    nix::pty::Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

fn set_size(master: &OwnedFd, cols: u16, rows: u16) -> std::io::Result<()> {
    let size = winsize(cols, rows);
    // SAFETY: TIOCSWINSZ reads one winsize from a valid pointer.
    let rc = unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ as _, &size) };
    if rc == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn close_on_exec(fd: &OwnedFd) -> std::io::Result<()> {
    let flags = FdFlag::from_bits_truncate(fcntl(fd, FcntlArg::F_GETFD)?);
    fcntl(fd, FcntlArg::F_SETFD(flags | FdFlag::FD_CLOEXEC))?;
    Ok(())
}

/// Registers a non-blocking pseudo-terminal end with the runtime's reactor.
fn watch(fd: OwnedFd) -> std::io::Result<AsyncFd<OwnedFd>> {
    // SAFETY: the `OwnedFd` owns an open descriptor and keeps it open and
    // unchanged for as long as the `AsyncFd` holds it.
    Ok(unsafe { AsyncFd::register(fd)? })
}

fn nonblocking(fd: OwnedFd) -> std::io::Result<OwnedFd> {
    let flags = OFlag::from_bits_truncate(fcntl(&fd, FcntlArg::F_GETFL)?);
    fcntl(&fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
    Ok(fd)
}

async fn read_some(master: &AsyncFd<OwnedFd>, buf: &mut [u8]) -> std::io::Result<usize> {
    loop {
        let mut ready = master.readable().await?;
        match ready.try_io(|fd| nix::unistd::read(fd.get_ref(), buf).map_err(std::io::Error::from)) {
            Ok(result) => return result,
            Err(_would_block) => continue,
        }
    }
}

async fn write_all(master: &AsyncFd<OwnedFd>, mut bytes: &[u8]) -> std::io::Result<()> {
    while !bytes.is_empty() {
        let mut ready = master.writable().await?;
        match ready.try_io(|fd| nix::unistd::write(fd.get_ref(), bytes).map_err(std::io::Error::from)) {
            Ok(Ok(n)) => bytes = &bytes[n..],
            Ok(Err(e)) => return Err(e),
            Err(_would_block) => continue,
        }
    }
    Ok(())
}

async fn publish(sink: &dyn FrameSink, id: &TerminalId, frame: &TerminalOutput) {
    if let Ok(payload) = serde_json::to_vec(frame) {
        sink.publish(terminal_output(id), Bytes::from(payload)).await;
    }
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
