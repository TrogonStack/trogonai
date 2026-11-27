use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_nats::Client;
use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tracing::warn;
use trogon_sandbox_contract::{
    LeaseId, TerminalArgument, TerminalDimension, TerminalError, TerminalId, TerminalOp, TerminalOutput,
    TerminalRequest, TerminalResponse,
};
use trogon_sandbox_protocol::subjects::{terminal_output, worker_lease_activity};
use trogon_sandbox_protocol::{
    MAX_TERMINAL_OUTPUT_CHUNK_BYTES, MAX_TERMINALS_PER_LEASE, terminal_data_frame, terminal_input_bytes,
};

use crate::command_line::build_command;
pub use crate::constants::DEFAULT_TERMINAL_IDLE_TIMEOUT;
use crate::constants::{DEFAULT_TERM, EXIT_CODE_WAIT, INPUT_WRITE_TIMEOUT};
use crate::hub_terminal::HubTerminal;
use crate::{GuestChannel, GuestError, GuestShell};

/// The interactive terminals one worker holds, across all its leases, each backed by a channel into the lease's
/// sandbox.
///
/// For each lease the worker keeps one guest session (a [`GuestShell`], which each worker implements) and opens one
/// channel per terminal, each with its own pseudo-terminal. Output frames go out on core NATS and the terminal's input
/// is the channel's data. A resize becomes a window change; closing a terminal closes its channel, which hangs up the
/// PTY.
///
/// Bounds: at most `MAX_TERMINALS_PER_LEASE` terminals per lease, input is written under a timeout and capped per
/// request by the protocol, output is forwarded in bounded chunks as it is read (nothing is queued), and a terminal
/// with no input or output for the idle timeout is closed. Terminal state lives in memory only.
pub struct TerminalHub {
    pub(crate) guest: Arc<dyn GuestShell>,
    pub(crate) client: Client,
    pub(crate) idle_timeout: Duration,
    terminals: Mutex<HashMap<TerminalId, Arc<HubTerminal>>>,
    /// Serializes `open` so the per-lease cap and the id check cannot race.
    open_gate: tokio::sync::Mutex<()>,
}

impl TerminalHub {
    pub fn new(guest: Arc<dyn GuestShell>, client: Client) -> Self {
        Self::with_idle_timeout(guest, client, DEFAULT_TERMINAL_IDLE_TIMEOUT)
    }

    pub fn with_idle_timeout(guest: Arc<dyn GuestShell>, client: Client, idle_timeout: Duration) -> Self {
        Self {
            guest,
            client,
            idle_timeout,
            terminals: Mutex::new(HashMap::new()),
            open_gate: tokio::sync::Mutex::new(()),
        }
    }

    /// Whether this worker holds `lease_id`.
    pub async fn holds(&self, lease_id: &LeaseId) -> bool {
        self.guest.holds(lease_id).await
    }

    /// Terminals currently open on `lease_id`.
    pub fn open_count(&self, lease_id: &LeaseId) -> usize {
        self.terminals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .filter(|terminal| terminal.lease_id == *lease_id)
            .count()
    }

    pub async fn handle(self: &Arc<Self>, request: &TerminalRequest) -> TerminalResponse {
        match &request.op {
            TerminalOp::Open {
                cols,
                rows,
                cmd,
                env,
                cwd,
                ..
            } => {
                self.open(
                    &request.lease_id,
                    &request.terminal_id,
                    (*cols, *rows),
                    cmd,
                    env,
                    cwd.as_deref(),
                )
                .await
            }
            TerminalOp::Input { data } => match self.owned(&request.lease_id, &request.terminal_id) {
                Err(error) => TerminalResponse::Failed(error),
                Ok(terminal) => {
                    terminal.touch();
                    // `TerminalInput` is validated base64, so the decode cannot fail.
                    let bytes = terminal_input_bytes(data).unwrap_or_default();
                    let mut input = terminal.input.lock().await;
                    let write = async {
                        input.write_all(&bytes).await?;
                        input.flush().await
                    };
                    match tokio::time::timeout(INPUT_WRITE_TIMEOUT, write).await {
                        Ok(Ok(())) => TerminalResponse::Done,
                        // The shell is gone or not reading: only close is useful.
                        _ => TerminalResponse::Failed(TerminalError::Exited),
                    }
                }
            },
            TerminalOp::Resize { cols, rows } => match self.owned(&request.lease_id, &request.terminal_id) {
                Err(error) => TerminalResponse::Failed(error),
                Ok(terminal) => {
                    terminal.touch();
                    match terminal.resize.send((*cols, *rows)).await {
                        Ok(()) => TerminalResponse::Done,
                        Err(_) => TerminalResponse::Failed(TerminalError::Exited),
                    }
                }
            },
            TerminalOp::Close => match self.owned(&request.lease_id, &request.terminal_id) {
                Err(TerminalError::NotFound) => TerminalResponse::Done,
                Err(error) => TerminalResponse::Failed(error),
                Ok(_) => {
                    self.finish(&request.terminal_id, None).await;
                    TerminalResponse::Done
                }
            },
        }
    }

    async fn open(
        self: &Arc<Self>,
        lease_id: &LeaseId,
        id: &TerminalId,
        size: (TerminalDimension, TerminalDimension),
        cmd: &[TerminalArgument],
        env: &BTreeMap<String, String>,
        cwd: Option<&str>,
    ) -> TerminalResponse {
        let _gate = self.open_gate.lock().await;
        {
            let terminals = self.terminals.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(existing) = terminals.get(id) {
                return if existing.lease_id == *lease_id {
                    TerminalResponse::Opened {
                        terminal_id: id.clone(),
                    }
                } else {
                    TerminalResponse::Failed(TerminalError::IdInUse)
                };
            }
            if terminals
                .values()
                .filter(|terminal| terminal.lease_id == *lease_id)
                .count()
                >= MAX_TERMINALS_PER_LEASE
            {
                return TerminalResponse::Failed(TerminalError::TooManyTerminals {
                    max: u32::try_from(MAX_TERMINALS_PER_LEASE).unwrap_or(u32::MAX),
                });
            }
        }
        if !self.guest.holds(lease_id).await {
            return TerminalResponse::Failed(TerminalError::SpawnFailed {
                reason: self.guest.not_held_reason().to_string(),
            });
        }
        let command = match build_command(cmd, cwd, env) {
            Ok(command) => command,
            Err(error) => {
                return TerminalResponse::Failed(TerminalError::InvalidRequest {
                    reason: error.to_string(),
                });
            }
        };
        let term = env.get("TERM").map_or(DEFAULT_TERM, String::as_str);
        let channel = match self.guest.open(lease_id, size, term, command).await {
            Ok(channel) => channel,
            Err(error) => {
                return TerminalResponse::Failed(TerminalError::SpawnFailed {
                    reason: error.to_string(),
                });
            }
        };
        let GuestChannel {
            stdin,
            stdout,
            resize,
            completion,
            close,
        } = channel;
        let terminal = Arc::new(HubTerminal::new(lease_id.clone(), stdin, resize, close));
        self.terminals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id.clone(), Arc::clone(&terminal));
        tokio::spawn(read_output(Arc::clone(self), id.clone(), terminal, stdout, completion));
        TerminalResponse::Opened {
            terminal_id: id.clone(),
        }
    }

    fn owned(&self, lease_id: &LeaseId, id: &TerminalId) -> Result<Arc<HubTerminal>, TerminalError> {
        match self
            .terminals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(id)
        {
            None => Err(TerminalError::NotFound),
            Some(terminal) if terminal.lease_id == *lease_id => Ok(Arc::clone(terminal)),
            Some(_) => Err(TerminalError::IdInUse),
        }
    }

    /// Ends `id` once: whoever removes it from the map sends the `Exit` frame. Closing the channel hangs up the PTY,
    /// which is a no-op when the shell already exited on its own.
    pub(crate) async fn finish(&self, id: &TerminalId, exit_code: Option<i32>) {
        let removed = self
            .terminals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(id);
        let Some(terminal) = removed else { return };
        terminal.stop_reader.cancel();
        terminal.close.cancel();
        let mut seq = terminal.seq.lock().await;
        publish_output(&self.client, id, &TerminalOutput::Exit { seq: *seq, exit_code }).await;
        *seq += 1;
    }

    /// Ends every terminal of a released lease and drops its guest session and ephemeral key.
    pub async fn close_lease(&self, lease_id: &LeaseId) {
        let ids: Vec<TerminalId> = self
            .terminals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|(_, terminal)| terminal.lease_id == *lease_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.finish(&id, None).await;
        }
        self.guest.release(lease_id).await;
    }
}

/// Publishes `payload` on `subject` and logs a failure: output and activity are best effort.
async fn publish_best_effort(client: &Client, subject: String, payload: Bytes) {
    if let Err(error) = client.publish(subject, payload).await {
        warn!(%error, "terminal publish failed");
    }
}

/// Publishes one output frame of terminal `id`.
async fn publish_output(client: &Client, id: &TerminalId, frame: &TerminalOutput) {
    let Ok(payload) = serde_json::to_vec(frame) else { return };
    publish_best_effort(client, terminal_output(id), Bytes::from(payload)).await;
}

/// Forwards a terminal's output until the stream ends, the terminal is closed, or it idles out, then ends the
/// terminal.
async fn read_output(
    hub: Arc<TerminalHub>,
    id: TerminalId,
    terminal: Arc<HubTerminal>,
    mut stdout: Pin<Box<dyn AsyncRead + Send>>,
    completion: Pin<Box<dyn Future<Output = Result<i32, GuestError>> + Send>>,
) {
    let mut buf = vec![0u8; MAX_TERMINAL_OUTPUT_CHUNK_BYTES];
    loop {
        let idle_left = hub.idle_timeout.saturating_sub(terminal.idle_for());
        let read = tokio::select! {
            _ = terminal.stop_reader.cancelled() => return,
            _ = tokio::time::sleep(idle_left) => {
                if terminal.idle_for() >= hub.idle_timeout {
                    hub.finish(&id, None).await;
                    return;
                }
                continue;
            }
            read = stdout.read(&mut buf) => read,
        };
        let read = match read {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        terminal.touch();
        {
            let mut seq = terminal.seq.lock().await;
            // The frame's text is base64 this crate just produced, so it always validates.
            let current = *seq;
            *seq += 1;
            for frame in terminal_data_frame(current, &buf[..read]).into_iter() {
                publish_output(&hub.client, &id, &frame).await;
            }
        }
        if terminal.activity.due(Instant::now()) {
            publish_best_effort(&hub.client, worker_lease_activity(&terminal.lease_id), Bytes::new()).await;
        }
    }
    // The stream ended on its own: the shell exited.
    let exit_code = tokio::time::timeout(EXIT_CODE_WAIT, completion)
        .await
        .ok()
        .and_then(Result::ok);
    hub.finish(&id, exit_code).await;
}
