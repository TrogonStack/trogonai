//! Interactive terminals for Firecracker guests.
//!
//! The guest agent owns the PTY (see `trogon_sandbox_actor_agent::terminal`) and streams
//! [`TerminalOutput`] frames over one long-lived vsock HTTP stream per terminal. This module is
//! the host half: it opens that stream, republishes every frame on core NATS under
//! [`trogon_sandbox_protocol::subjects::terminal_output`], and relays input, resize and close to the
//! guest. The wire contract is the one every other provider serves (see
//! `trogon_sandbox_contract`); nothing here adds a protocol.
//!
//! The vsock stream is the terminal's lifetime. Closing it, whether on `close`, on worker
//! shutdown or because the worker died, makes the guest hang the shell up, so a terminal never
//! outlives the worker's interest in it. Output backpressure is the PTY: the pump publishes to
//! core NATS without waiting on readers (a slow reader loses frames, visible as a `seq` gap,
//! exactly as on the other providers), and the guest never drops a frame itself.
//!
//! Frames carry the guest's `seq` unchanged. A stream that ends without an `Exit` frame (the
//! guest or the VM went away) is closed with one synthesized `Exit` whose code is unknown, so
//! the "exactly one `Exit`" promise holds on this provider too.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use trogon_sandbox_actor_agent::TerminalFrame;
use trogon_sandbox_contract::{
    LeaseId, TerminalError, TerminalId, TerminalOp, TerminalOutput, TerminalRequest, TerminalResponse,
};
use trogon_sandbox_protocol::subjects::{terminal_output, worker_lease_activity};
use trogon_sandbox_protocol::{
    ActivitySignal, MAX_TERMINALS_PER_LEASE, TerminalReplayRing, terminal_output_bytes, terminal_output_seq,
};
use trogon_std::env::ReadEnv;

use crate::agent::{GuestAgent, TerminalStream};

/// Where output frames and activity signals go. The worker publishes on core NATS; tests
/// collect.
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

struct Entry {
    lease_id: LeaseId,
    stop: CancellationToken,
    /// The latest output, for a reader that reattaches. Held across each
    /// publish so a replay never interleaves with a live frame.
    replay: tokio::sync::Mutex<TerminalReplayRing>,
}

/// The terminals one Firecracker worker holds, across its leases.
pub struct FirecrackerTerminals {
    sink: Arc<dyn FrameSink>,
    terminals: Mutex<HashMap<TerminalId, Arc<Entry>>>,
    /// Serializes `open` so the per-lease cap and the id check cannot race.
    open_gate: tokio::sync::Mutex<()>,
    /// Size of each new terminal's replay ring, in bytes.
    replay_bytes: usize,
}

impl FirecrackerTerminals {
    /// Terminals whose replay ring is sized by
    /// [`trogon_sandbox_protocol::TERMINAL_REPLAY_BYTES_ENV`] in `env`.
    pub fn new(sink: Arc<dyn FrameSink>, env: &impl ReadEnv) -> Arc<Self> {
        Self::with_replay_bytes(sink, trogon_sandbox_protocol::terminal_replay_bytes_from_env(env))
    }

    /// Terminals with a replay ring of `replay_bytes` each (zero: no replay).
    pub fn with_replay_bytes(sink: Arc<dyn FrameSink>, replay_bytes: usize) -> Arc<Self> {
        Arc::new(Self {
            replay_bytes,
            sink,
            terminals: Mutex::new(HashMap::new()),
            open_gate: tokio::sync::Mutex::new(()),
        })
    }

    fn table(&self) -> std::sync::MutexGuard<'_, HashMap<TerminalId, Arc<Entry>>> {
        self.terminals.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn open_count(&self) -> usize {
        self.table().len()
    }

    /// Applies one request against the guest `agent` of the lease it names.
    pub async fn handle(self: &Arc<Self>, agent: &GuestAgent, req: &TerminalRequest) -> TerminalResponse {
        match &req.op {
            TerminalOp::Open { .. } => self.open(agent, req).await,
            TerminalOp::Close => match self.owned(&req.lease_id, &req.terminal_id) {
                Err(TerminalError::NotFound) => TerminalResponse::Done,
                Err(e) => TerminalResponse::Failed(e),
                Ok(entry) => {
                    // The guest hangs the shell up and the stream then ends with its `Exit`.
                    // If the guest cannot be told, dropping the stream does the same.
                    if agent.terminal_op(req).await.is_err() {
                        entry.stop.cancel();
                    }
                    TerminalResponse::Done
                }
            },
            TerminalOp::Input { .. } | TerminalOp::Resize { .. } => {
                if let Err(e) = self.owned(&req.lease_id, &req.terminal_id) {
                    return TerminalResponse::Failed(e);
                }
                match agent.terminal_op(req).await {
                    Ok(response) => response,
                    Err(e) => {
                        warn!(error = %e, "terminal request to the guest failed");
                        TerminalResponse::Failed(TerminalError::Exited)
                    }
                }
            }
        }
    }

    fn owned(&self, lease: &LeaseId, id: &TerminalId) -> Result<Arc<Entry>, TerminalError> {
        match self.table().get(id) {
            None => Err(TerminalError::NotFound),
            Some(e) if e.lease_id == *lease => Ok(Arc::clone(e)),
            // Another lease's terminal never reads as this lease's.
            Some(_) => Err(TerminalError::IdInUse),
        }
    }

    async fn open(self: &Arc<Self>, agent: &GuestAgent, req: &TerminalRequest) -> TerminalResponse {
        let _gate = self.open_gate.lock().await;
        let existing = self.table().get(&req.terminal_id).cloned();
        if let Some(existing) = existing {
            if existing.lease_id != req.lease_id {
                return TerminalResponse::Failed(TerminalError::IdInUse);
            }
            let resume_from = match &req.op {
                TerminalOp::Open { resume_from, .. } => *resume_from,
                _ => None,
            };
            // Frames the reader missed, ahead of whatever the pump sends next.
            let frames = existing
                .replay
                .lock()
                .await
                .reattach_frames(resume_from)
                .unwrap_or_default();
            for frame in frames {
                self.publish(&req.terminal_id, &frame).await;
            }
            return TerminalResponse::Opened {
                terminal_id: req.terminal_id.clone(),
            };
        }
        {
            let map = self.table();
            if map.values().filter(|t| t.lease_id == req.lease_id).count() >= MAX_TERMINALS_PER_LEASE {
                return TerminalResponse::Failed(TerminalError::TooManyTerminals {
                    max: MAX_TERMINALS_PER_LEASE as u32,
                });
            }
        }
        let (response, stream) = match agent.terminal_open(req).await {
            Ok(opened) => opened,
            Err(e) => {
                return TerminalResponse::Failed(TerminalError::SpawnFailed { reason: e.to_string() });
            }
        };
        let Some(stream) = stream else {
            return response;
        };
        let entry = Arc::new(Entry {
            lease_id: req.lease_id.clone(),
            stop: CancellationToken::new(),
            replay: tokio::sync::Mutex::new(TerminalReplayRing::new(self.replay_bytes)),
        });
        self.table().insert(req.terminal_id.clone(), Arc::clone(&entry));
        tokio::spawn(Arc::clone(self).pump(req.terminal_id.clone(), entry, stream));
        response
    }

    /// Republishes the guest's frames until the terminal ends, then forgets it.
    async fn pump(self: Arc<Self>, id: TerminalId, entry: Arc<Entry>, mut stream: TerminalStream) {
        let activity = ActivitySignal::default();
        let lease = entry.lease_id.clone();
        let mut next_seq = 0u64;
        loop {
            let frame = tokio::select! {
                () = entry.stop.cancelled() => None,
                frame = stream.next_frame() => frame.ok().flatten(),
            };
            match frame {
                Some(TerminalFrame::Output(output)) => {
                    next_seq = terminal_output_seq(&output) + 1;
                    let ended = matches!(output, TerminalOutput::Exit { .. });
                    {
                        let mut replay = entry.replay.lock().await;
                        if let Some(bytes) = terminal_output_bytes(&output) {
                            replay.push(terminal_output_seq(&output), &bytes);
                        }
                        self.publish(&id, &output).await;
                    }
                    if ended {
                        break;
                    }
                    if activity.due(Instant::now()) {
                        self.sink.publish(worker_lease_activity(&lease), Bytes::new()).await;
                    }
                }
                // A response frame mid-stream is not part of the contract.
                Some(TerminalFrame::Response(_)) => {}
                None => {
                    // The stream ended without an `Exit`: the guest or the VM is gone.
                    let exit = TerminalOutput::Exit {
                        seq: next_seq,
                        exit_code: None,
                    };
                    self.publish(&id, &exit).await;
                    break;
                }
            }
        }
        self.table().remove(&id);
    }

    async fn publish(&self, id: &TerminalId, frame: &TerminalOutput) {
        if let Ok(payload) = serde_json::to_vec(frame) {
            self.sink.publish(terminal_output(id), Bytes::from(payload)).await;
        }
    }
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
