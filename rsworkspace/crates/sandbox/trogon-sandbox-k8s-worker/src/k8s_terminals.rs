//! Interactive terminals backed by a pod exec with a pseudo-TTY.
//!
//! One exec per terminal, started through the Kubernetes exec API with `tty`
//! set (so the apiserver allocates the PTY and accepts resize messages). Output
//! frames go out on core NATS and the terminal's input is the exec's attached
//! stdin. The exec API cannot signal a process, so the command is started
//! through a tiny wrapper that records its pid in the pod and `close` signals
//! that pid.
//!
//! Bounds: at most [`MAX_TERMINALS_PER_LEASE`] terminals per lease, input is
//! written under a timeout and capped per request by the protocol, output is
//! forwarded in bounded chunks as it is read (nothing is queued), the resize
//! queue holds a handful of sizes, and a terminal with no input or output for
//! the idle timeout is closed. Terminal state lives in memory only; nothing is
//! persisted.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_nats::Client;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    LeaseId, TerminalError, TerminalId, TerminalInput, TerminalOp, TerminalOutput, TerminalRequest, TerminalResponse,
};
use trogon_sandbox_protocol::{ActivitySignal, MAX_TERMINALS_PER_LEASE, TerminalReplayRing, terminal_input_bytes};
use trogon_sandbox_providers::TtyPodExec;

use crate::TerminalBackend;
use crate::constants::{DEFAULT_IDLE_TIMEOUT, INPUT_WRITE_TIMEOUT};
use crate::terminal_commands::{pid_file, wrapped_command};
use crate::terminal_state::TerminalState;
use terminal_io::{publish, read_output};

mod terminal_io;

fn done_or_failed(result: Result<(), TerminalError>) -> TerminalResponse {
    match result {
        Ok(()) => TerminalResponse::Done,
        Err(error) => TerminalResponse::Failed(error),
    }
}

/// The terminals one k8s worker holds, across all its leases.
pub struct K8sTerminals {
    backend: Arc<dyn TerminalBackend>,
    client: Client,
    idle_timeout: Duration,
    terminals: Mutex<HashMap<TerminalId, Arc<TerminalState>>>,
    /// Serializes `open` so the per-lease cap and the id check cannot race.
    open_gate: tokio::sync::Mutex<()>,
    /// Size of each new terminal's replay ring, in bytes.
    replay_bytes: usize,
}

impl K8sTerminals {
    pub fn new(backend: Arc<dyn TerminalBackend>, client: Client, replay_bytes: usize) -> Self {
        Self::with_idle_timeout(backend, client, replay_bytes, DEFAULT_IDLE_TIMEOUT)
    }

    pub fn with_idle_timeout(
        backend: Arc<dyn TerminalBackend>,
        client: Client,
        replay_bytes: usize,
        idle_timeout: Duration,
    ) -> Self {
        Self {
            backend,
            client,
            idle_timeout,
            terminals: Mutex::new(HashMap::new()),
            open_gate: tokio::sync::Mutex::new(()),
            replay_bytes,
        }
    }

    pub(crate) fn idle_timeout(&self) -> Duration {
        self.idle_timeout
    }

    pub(crate) fn client(&self) -> &Client {
        &self.client
    }

    fn table(&self) -> std::sync::MutexGuard<'_, HashMap<TerminalId, Arc<TerminalState>>> {
        self.terminals.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether this worker holds `lease_id`.
    pub async fn holds(&self, lease_id: &LeaseId) -> bool {
        self.backend.pod_for(lease_id).await.is_some()
    }

    /// Terminals currently open on `lease_id`.
    pub fn open_count(&self, lease_id: &LeaseId) -> usize {
        self.table().values().filter(|t| &t.lease_id == lease_id).count()
    }

    pub async fn handle(self: &Arc<Self>, body: &[u8]) -> TerminalResponse {
        match serde_json::from_slice::<TerminalRequest>(body) {
            Ok(request) => self.handle_request(&request).await,
            Err(error) => TerminalResponse::Failed(TerminalError::InvalidRequest {
                reason: error.to_string(),
            }),
        }
    }

    pub async fn handle_request(self: &Arc<Self>, req: &TerminalRequest) -> TerminalResponse {
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
                let size = (cols.get(), rows.get());
                let cmd: Vec<String> = cmd.iter().map(|argument| argument.as_str().to_string()).collect();
                self.open(lease, id, size, &cmd, env, cwd.as_deref(), *resume_from)
                    .await
            }
            TerminalOp::Input { data } => done_or_failed(self.write_input(lease, id, data).await),
            TerminalOp::Resize { cols, rows } => done_or_failed(self.resize(lease, id, (cols.get(), rows.get())).await),
            TerminalOp::Close => match self.owned(lease, id) {
                Err(TerminalError::NotFound) => TerminalResponse::Done,
                Err(e) => TerminalResponse::Failed(e),
                Ok(_) => {
                    self.finish(id, None, true).await;
                    TerminalResponse::Done
                }
            },
        }
    }

    async fn write_input(&self, lease: &LeaseId, id: &TerminalId, data: &TerminalInput) -> Result<(), TerminalError> {
        let bytes = terminal_input_bytes(data).map_err(|e| TerminalError::InvalidRequest { reason: e.to_string() })?;
        let terminal = self.owned(lease, id)?;
        terminal.touch();
        let mut input = terminal.input.lock().await;
        let write = async {
            input.write_all(&bytes).await?;
            input.flush().await
        };
        // The process is gone or not reading: only close is useful.
        match tokio::time::timeout(INPUT_WRITE_TIMEOUT, write).await {
            Ok(Ok(())) => Ok(()),
            _ => Err(TerminalError::Exited),
        }
    }

    async fn resize(&self, lease: &LeaseId, id: &TerminalId, size: (u16, u16)) -> Result<(), TerminalError> {
        let terminal = self.owned(lease, id)?;
        terminal.touch();
        terminal.resize.send(size).await.map_err(|_| TerminalError::Exited)
    }

    #[allow(clippy::too_many_arguments)]
    async fn open(
        self: &Arc<Self>,
        lease_id: &LeaseId,
        id: &TerminalId,
        size: (u16, u16),
        cmd: &[String],
        env: &BTreeMap<String, String>,
        cwd: Option<&str>,
        resume_from: Option<u64>,
    ) -> TerminalResponse {
        let _gate = self.open_gate.lock().await;
        let existing = self.table().get(id).cloned();
        if let Some(existing) = existing {
            if &existing.lease_id != lease_id {
                return TerminalResponse::Failed(TerminalError::IdInUse);
            }
            self.reattach(id, &existing, resume_from).await;
            return TerminalResponse::Opened {
                terminal_id: id.clone(),
            };
        }
        if self.open_count(lease_id) >= MAX_TERMINALS_PER_LEASE {
            return TerminalResponse::Failed(TerminalError::TooManyTerminals {
                max: u32::try_from(MAX_TERMINALS_PER_LEASE).unwrap_or(u32::MAX),
            });
        }
        let Some(pod) = self.backend.pod_for(lease_id).await else {
            return TerminalResponse::Failed(TerminalError::SpawnFailed {
                reason: "the worker holds no pod for this lease".to_string(),
            });
        };
        let exec = match self.backend.start(&pod, wrapped_command(id, cmd, cwd, env), size).await {
            Ok(e) => e,
            Err(e) => {
                return TerminalResponse::Failed(TerminalError::SpawnFailed { reason: e.to_string() });
            }
        };
        let TtyPodExec {
            stdin,
            stdout,
            resize,
            completion,
        } = exec;
        let terminal = Arc::new(TerminalState {
            lease_id: lease_id.clone(),
            pod,
            input: tokio::sync::Mutex::new(stdin),
            resize,
            seq: tokio::sync::Mutex::new(0),
            replay: Mutex::new(TerminalReplayRing::new(self.replay_bytes)),
            stop_reader: CancellationToken::new(),
            activity: ActivitySignal::default(),
            last_io: Mutex::new(Instant::now()),
        });
        self.table().insert(id.clone(), Arc::clone(&terminal));
        let completion = Box::pin(async move { completion.await.map_err(crate::K8sWorkerError::from) });
        tokio::spawn(read_output(Arc::clone(self), id.clone(), terminal, stdout, completion));
        TerminalResponse::Opened {
            terminal_id: id.clone(),
        }
    }

    /// Republishes what a reader that reattached missed (see
    /// [`TerminalReplayRing::reattach_frames`]). Holding `seq` keeps the
    /// replay ahead of any live frame.
    async fn reattach(&self, id: &TerminalId, terminal: &TerminalState, resume_from: Option<u64>) {
        let _seq = terminal.seq.lock().await;
        let frames = terminal
            .replay
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reattach_frames(resume_from);
        let frames = match frames {
            Ok(frames) => frames,
            Err(error) => {
                tracing::warn!(%error, "terminal replay did not encode");
                return;
            }
        };
        for frame in frames {
            publish(&self.client, id, &frame).await;
        }
    }

    fn owned(&self, lease_id: &LeaseId, id: &TerminalId) -> Result<Arc<TerminalState>, TerminalError> {
        match self.table().get(id) {
            None => Err(TerminalError::NotFound),
            Some(t) if &t.lease_id == lease_id => Ok(Arc::clone(t)),
            Some(_) => Err(TerminalError::IdInUse),
        }
    }

    /// Ends `id` once: whoever removes it from the map sends the `Exit` frame.
    /// `kill` signals the command first (a close or an idle timeout; an exit
    /// that happened on its own needs no signal).
    pub(crate) async fn finish(&self, id: &TerminalId, exit_code: Option<i32>, kill: bool) {
        let Some(terminal) = self.table().remove(id) else {
            return;
        };
        terminal.stop_reader.cancel();
        if kill {
            self.backend.hangup(&terminal.pod, &pid_file(id)).await;
        }
        let mut seq = terminal.seq.lock().await;
        publish(&self.client, id, &TerminalOutput::Exit { seq: *seq, exit_code }).await;
        *seq += 1;
    }

    /// Ends every terminal of a released lease. The pod is torn down right
    /// after, so no signal is sent.
    pub async fn close_lease(&self, lease_id: &LeaseId) {
        let ids: Vec<TerminalId> = self
            .table()
            .iter()
            .filter(|(_, t)| &t.lease_id == lease_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.finish(&id, None, false).await;
        }
    }
}
