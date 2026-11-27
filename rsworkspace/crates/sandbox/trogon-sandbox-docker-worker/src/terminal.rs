//! Interactive terminals backed by a Docker exec with a pseudo-TTY.
//!
//! One exec per terminal. Output frames go out on core NATS (see the NATS notes in this module) and the terminal's input is the exec's
//! attached stdin. Docker has no API to kill an exec, so the command is
//! started through a tiny wrapper that records its pid in the container and
//! `close` signals that pid.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_nats::Client;
use bollard::Docker;
use bollard::exec::{CreateExecOptions, ResizeExecOptions, StartExecOptions, StartExecResults};
use bytes::Bytes;
use futures::StreamExt;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use tracing::warn;
use trogon_sandbox_contract::{
    LeaseId, TerminalArgument, TerminalDimension, TerminalError, TerminalId, TerminalOp, TerminalOutput,
    TerminalRequest, TerminalResponse,
};
use trogon_sandbox_protocol::subjects::{terminal_output, worker_lease_activity};
use trogon_sandbox_protocol::{
    ActivitySignal, MAX_TERMINAL_OUTPUT_CHUNK_BYTES, MAX_TERMINALS_PER_LEASE, TerminalReplayRing, terminal_data_frame,
    terminal_input_bytes, terminal_replay_bytes_from_env,
};
use trogon_std::env::SystemEnv;

use crate::adapter::DockerWorkerProvider;

mod constants;

use constants::{DEFAULT_TERM, INPUT_WRITE_TIMEOUT};

type ExecInput = Pin<Box<dyn AsyncWrite + Send>>;

struct Terminal {
    lease_id: LeaseId,
    exec_id: String,
    docker: Docker,
    container_id: String,
    input: tokio::sync::Mutex<ExecInput>,
    /// Next output `seq`. Held across the publish so frames leave in order.
    seq: tokio::sync::Mutex<u64>,
    /// The latest output, for a reader that reattaches. Only touched with
    /// `seq` held, so it always matches what was published.
    replay: Mutex<TerminalReplayRing>,
    stop_reader: CancellationToken,
    /// Paces the activity signal output sends, so a terminal that only
    /// prints (a build, `tail -f`) still counts toward the idle timeout.
    activity: ActivitySignal,
}

/// The terminals one docker worker holds, across all its leases.
pub struct DockerTerminals {
    terminals: Mutex<HashMap<TerminalId, Arc<Terminal>>>,
    /// Serializes `open` so the per-lease cap and the id check cannot race.
    open_gate: tokio::sync::Mutex<()>,
    /// Size of each new terminal's replay ring, in bytes.
    replay_bytes: usize,
}

impl Default for DockerTerminals {
    /// Replay rings are sized by [`trogon_sandbox_protocol::TERMINAL_REPLAY_BYTES_ENV`].
    fn default() -> Self {
        Self::with_replay_bytes(terminal_replay_bytes_from_env(&SystemEnv))
    }
}

/// Where the wrapper records the command's pid inside the container.
fn pid_file(id: &TerminalId) -> String {
    format!("/tmp/.sbx-term-{id}")
}

/// The argv that starts `cmd` (or a shell) and records its pid first. `exec`
/// keeps the pid, so signalling it signals the command itself.
fn wrapped_command(id: &TerminalId, cmd: &[TerminalArgument]) -> Vec<String> {
    let mut argv = vec![
        "sh".to_string(),
        "-c".to_string(),
        format!("echo $$ > {}; exec \"$@\"", pid_file(id)),
        "sh".to_string(),
    ];
    if cmd.is_empty() {
        argv.push("/bin/sh".to_string());
    } else {
        argv.extend(cmd.iter().map(|argument| argument.as_str().to_owned()));
    }
    argv
}

fn exec_env(env: &std::collections::BTreeMap<String, String>) -> Vec<String> {
    let mut out: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    if !env.contains_key("TERM") {
        out.push(format!("TERM={DEFAULT_TERM}"));
    }
    out
}

impl DockerTerminals {
    /// Terminals with a replay ring of `replay_bytes` each (zero: no replay).
    pub fn with_replay_bytes(replay_bytes: usize) -> Self {
        Self {
            terminals: Mutex::new(HashMap::new()),
            open_gate: tokio::sync::Mutex::new(()),
            replay_bytes,
        }
    }

    pub async fn handle(
        self: &Arc<Self>,
        provider: &DockerWorkerProvider,
        client: &Client,
        body: &[u8],
    ) -> TerminalResponse {
        let req = match serde_json::from_slice::<TerminalRequest>(body) {
            Ok(r) => r,
            Err(e) => {
                return TerminalResponse::Failed(TerminalError::InvalidRequest { reason: e.to_string() });
            }
        };
        let id = &req.terminal_id;
        let lease = &req.lease_id;
        match &req.op {
            TerminalOp::Open {
                cols,
                rows,
                cmd,
                env,
                cwd,
                resume_from,
            } => {
                self.open(
                    provider,
                    client,
                    &req.lease_id,
                    &req.terminal_id,
                    (*cols, *rows),
                    cmd,
                    env,
                    cwd.clone(),
                    *resume_from,
                )
                .await
            }
            TerminalOp::Input { data } => {
                let bytes = match terminal_input_bytes(data) {
                    Ok(b) => b,
                    Err(e) => {
                        return TerminalResponse::Failed(TerminalError::InvalidRequest { reason: e.to_string() });
                    }
                };
                match self.owned(lease, id) {
                    Err(e) => TerminalResponse::Failed(e),
                    Ok(t) => {
                        let mut input = t.input.lock().await;
                        let write = async {
                            input.write_all(&bytes).await?;
                            input.flush().await
                        };
                        match tokio::time::timeout(INPUT_WRITE_TIMEOUT, write).await {
                            Ok(Ok(())) => TerminalResponse::Done,
                            // The process is gone or not reading: only close is useful.
                            _ => TerminalResponse::Failed(TerminalError::Exited),
                        }
                    }
                }
            }
            TerminalOp::Resize { cols, rows } => match self.owned(lease, id) {
                Err(e) => TerminalResponse::Failed(e),
                Ok(t) => match resize(&t, cols.get(), rows.get()).await {
                    Ok(()) => TerminalResponse::Done,
                    Err(e) => {
                        warn!(error = %e, "terminal resize failed");
                        TerminalResponse::Failed(TerminalError::Exited)
                    }
                },
            },
            TerminalOp::Close => match self.owned(lease, id) {
                Err(TerminalError::NotFound) => TerminalResponse::Done,
                Err(e) => TerminalResponse::Failed(e),
                Ok(_) => {
                    self.finish(client, &req.terminal_id, None, true).await;
                    TerminalResponse::Done
                }
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn open(
        self: &Arc<Self>,
        provider: &DockerWorkerProvider,
        client: &Client,
        lease_id: &LeaseId,
        id: &TerminalId,
        size: (TerminalDimension, TerminalDimension),
        cmd: &[TerminalArgument],
        env: &std::collections::BTreeMap<String, String>,
        cwd: Option<String>,
        resume_from: Option<u64>,
    ) -> TerminalResponse {
        let _gate = self.open_gate.lock().await;
        let existing = self.held().get(id).cloned();
        if let Some(existing) = existing {
            if &existing.lease_id != lease_id {
                return TerminalResponse::Failed(TerminalError::IdInUse);
            }
            reattach(client, id, &existing, resume_from).await;
            return TerminalResponse::Opened {
                terminal_id: id.clone(),
            };
        }
        {
            let map = self.held();
            if map.values().filter(|t| &t.lease_id == lease_id).count() >= MAX_TERMINALS_PER_LEASE {
                return TerminalResponse::Failed(TerminalError::TooManyTerminals {
                    max: MAX_TERMINALS_PER_LEASE as u32,
                });
            }
        }
        let Some((docker, container_id)) = provider.terminal_target(lease_id).await else {
            return TerminalResponse::Failed(TerminalError::SpawnFailed {
                reason: "the worker holds no container for this lease".to_string(),
            });
        };
        let spawn_failed = |reason: String| TerminalResponse::Failed(TerminalError::SpawnFailed { reason });
        let exec = match docker
            .create_exec(
                &container_id,
                CreateExecOptions {
                    attach_stdin: Some(true),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    tty: Some(true),
                    cmd: Some(wrapped_command(id, cmd)),
                    env: Some(exec_env(env)),
                    working_dir: cwd,
                    ..Default::default()
                },
            )
            .await
        {
            Ok(e) => e,
            Err(e) => return spawn_failed(e.to_string()),
        };
        let started = docker
            .start_exec(
                &exec.id,
                Some(StartExecOptions {
                    detach: false,
                    tty: true,
                    output_capacity: Some(MAX_TERMINAL_OUTPUT_CHUNK_BYTES),
                }),
            )
            .await;
        let (output, input) = match started {
            Ok(StartExecResults::Attached { output, input }) => (output, input),
            Ok(StartExecResults::Detached) => return spawn_failed("exec detached".to_string()),
            Err(e) => return spawn_failed(e.to_string()),
        };
        let terminal = Arc::new(Terminal {
            lease_id: lease_id.clone(),
            exec_id: exec.id,
            docker,
            container_id,
            input: tokio::sync::Mutex::new(input),
            seq: tokio::sync::Mutex::new(0),
            replay: Mutex::new(TerminalReplayRing::new(self.replay_bytes)),
            stop_reader: CancellationToken::new(),
            activity: ActivitySignal::default(),
        });
        if let Err(e) = resize(&terminal, size.0.get(), size.1.get()).await {
            warn!(error = %e, "initial terminal resize failed");
        }
        self.held().insert(id.clone(), Arc::clone(&terminal));
        tokio::spawn(read_output(
            Arc::clone(self),
            client.clone(),
            id.clone(),
            terminal,
            output,
        ));
        TerminalResponse::Opened {
            terminal_id: id.clone(),
        }
    }

    fn held(&self) -> MutexGuard<'_, HashMap<TerminalId, Arc<Terminal>>> {
        self.terminals.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn owned(&self, lease_id: &LeaseId, id: &TerminalId) -> Result<Arc<Terminal>, TerminalError> {
        match self.held().get(id) {
            None => Err(TerminalError::NotFound),
            Some(t) if &t.lease_id == lease_id => Ok(Arc::clone(t)),
            Some(_) => Err(TerminalError::IdInUse),
        }
    }

    /// Ends `id` once: whoever removes it from the map sends the `Exit` frame.
    /// `kill` signals the command first (a close; an exit that happened on
    /// its own needs no signal).
    async fn finish(&self, client: &Client, id: &TerminalId, exit_code: Option<i32>, kill: bool) {
        let Some(terminal) = self.held().remove(id) else {
            return;
        };
        terminal.stop_reader.cancel();
        if kill {
            kill_command(&terminal, id).await;
        }
        let exit_code = match exit_code {
            Some(c) => Some(c),
            None if kill => None,
            None => exit_code_of(&terminal).await,
        };
        let mut seq = terminal.seq.lock().await;
        publish(client, id, &TerminalOutput::Exit { seq: *seq, exit_code }).await;
        *seq += 1;
    }

    /// Ends every terminal of a released lease. The container is torn down
    /// right after, so no signal is sent.
    pub async fn close_lease(&self, client: &Client, lease_id: &LeaseId) {
        let ids: Vec<TerminalId> = self
            .held()
            .iter()
            .filter(|(_, t)| &t.lease_id == lease_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.finish(client, &id, None, false).await;
        }
    }
}

async fn resize(t: &Terminal, cols: u16, rows: u16) -> Result<(), bollard::errors::Error> {
    t.docker
        .resize_exec(
            &t.exec_id,
            ResizeExecOptions {
                height: rows,
                width: cols,
            },
        )
        .await
}

async fn exit_code_of(t: &Terminal) -> Option<i32> {
    let inspect = t.docker.inspect_exec(&t.exec_id).await.ok()?;
    inspect.exit_code.and_then(|c| i32::try_from(c).ok())
}

/// Hangs up the command. Best effort: a command that already exited, or a
/// container without `sh`, simply leaves nothing to signal.
async fn kill_command(t: &Terminal, id: &TerminalId) {
    let file = pid_file(id);
    let script = format!("kill -HUP \"$(cat {file})\" 2>/dev/null; rm -f {file}");
    let created = t
        .docker
        .create_exec(
            &t.container_id,
            CreateExecOptions {
                cmd: Some(vec!["sh".to_string(), "-c".to_string(), script]),
                ..Default::default()
            },
        )
        .await;
    if let Ok(exec) = created {
        let _ = t
            .docker
            .start_exec(
                &exec.id,
                Some(StartExecOptions {
                    detach: true,
                    ..Default::default()
                }),
            )
            .await;
    }
}

async fn read_output(
    hub: Arc<DockerTerminals>,
    client: Client,
    id: TerminalId,
    terminal: Arc<Terminal>,
    mut output: Pin<
        Box<dyn futures::Stream<Item = Result<bollard::container::LogOutput, bollard::errors::Error>> + Send>,
    >,
) {
    loop {
        let next = tokio::select! {
            _ = terminal.stop_reader.cancelled() => return,
            next = output.next() => next,
        };
        let Some(Ok(chunk)) = next else { break };
        let bytes = chunk.into_bytes();
        for part in bytes.chunks(MAX_TERMINAL_OUTPUT_CHUNK_BYTES) {
            let mut seq = terminal.seq.lock().await;
            terminal
                .replay
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(*seq, part);
            if let Ok(frame) = terminal_data_frame(*seq, part) {
                publish(&client, &id, &frame).await;
            }
            *seq += 1;
        }
        if terminal.activity.due(std::time::Instant::now()) {
            let subject = worker_lease_activity(&terminal.lease_id);
            if let Err(e) = client.publish(subject, Bytes::new()).await {
                warn!(error = %e, "terminal activity signal publish failed");
            }
        }
    }
    // The stream ended on its own: the command exited.
    hub.finish(&client, &id, None, false).await;
}

/// Republishes what a reader that reattached missed (see
/// [`TerminalReplayRing::reattach_frames`]). Holding `seq` keeps the replay
/// ahead of any live frame.
async fn reattach(client: &Client, id: &TerminalId, terminal: &Terminal, resume_from: Option<u64>) {
    let _seq = terminal.seq.lock().await;
    let frames = terminal
        .replay
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .reattach_frames(resume_from)
        .unwrap_or_default();
    for frame in frames {
        publish(client, id, &frame).await;
    }
}

async fn publish(client: &Client, id: &TerminalId, frame: &TerminalOutput) {
    if let Ok(payload) = serde_json::to_vec(frame)
        && let Err(e) = client.publish(terminal_output(id), Bytes::from(payload)).await
    {
        warn!(error = %e, "terminal output publish failed");
    }
}

#[cfg(test)]
mod tests;
