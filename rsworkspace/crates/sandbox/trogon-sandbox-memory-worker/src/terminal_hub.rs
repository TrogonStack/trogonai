//! Echo terminals for the memory worker.
//!
//! A terminal here has no process: it prints a prompt on open and echoes every
//! input byte back (a carriage return also gets a line feed), the way a
//! cooked-mode PTY with no program behind it would. That is enough to test the
//! whole path (broker, gateway, client) without a container runtime. A resize
//! prints the new size so tests can observe it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use trogon_sandbox_contract::{
    LeaseId, TerminalError, TerminalId, TerminalOp, TerminalOutput, TerminalRequest, TerminalResponse,
};
use trogon_sandbox_protocol::subjects::terminal_output;
use trogon_sandbox_protocol::{
    MAX_TERMINAL_OUTPUT_CHUNK_BYTES, MAX_TERMINALS_PER_LEASE, terminal_data_frame, terminal_input_bytes,
};

use crate::FrameSink;
use crate::mutex_ext::MutexExt;

struct Terminal {
    lease_id: LeaseId,
    /// Next output `seq`. Held across the publish so frames leave in order
    /// even when requests are handled on separate tasks.
    seq: tokio::sync::Mutex<u64>,
}

/// The echo terminals of every lease a worker holds.
#[derive(Default)]
pub struct TerminalHub {
    terminals: Mutex<HashMap<TerminalId, Arc<Terminal>>>,
}

impl TerminalHub {
    /// Serves one terminal request that arrived on the subject of `lease_id`.
    pub async fn handle(&self, sink: &dyn FrameSink, lease_id: &LeaseId, body: &[u8]) -> TerminalResponse {
        let request = match serde_json::from_slice::<TerminalRequest>(body) {
            Ok(request) => request,
            Err(error) => {
                return TerminalResponse::Failed(TerminalError::InvalidRequest {
                    reason: error.to_string(),
                });
            }
        };
        if request.lease_id != *lease_id {
            return TerminalResponse::Failed(TerminalError::InvalidRequest {
                reason: "lease id does not match the subject".to_string(),
            });
        }
        let id = &request.terminal_id;
        match &request.op {
            TerminalOp::Open { .. } => self.open(sink, lease_id, id).await,
            TerminalOp::Input { data } => match self.owned(lease_id, id) {
                Err(error) => TerminalResponse::Failed(error),
                Ok(terminal) => {
                    // The input was validated as base64 when the request parsed.
                    let bytes = terminal_input_bytes(data).unwrap_or_default();
                    let mut echoed = Vec::with_capacity(bytes.len());
                    for byte in bytes {
                        echoed.push(byte);
                        if byte == b'\r' {
                            echoed.push(b'\n');
                        }
                    }
                    emit(sink, id, &terminal, &echoed).await;
                    TerminalResponse::Done
                }
            },
            TerminalOp::Resize { cols, rows } => match self.owned(lease_id, id) {
                Err(error) => TerminalResponse::Failed(error),
                Ok(terminal) => {
                    let line = format!("[resized {cols}x{rows}]\r\n");
                    emit(sink, id, &terminal, line.as_bytes()).await;
                    TerminalResponse::Done
                }
            },
            TerminalOp::Close => match self.owned(lease_id, id) {
                // A retried close, or a close of an unknown id, is not an error.
                Err(TerminalError::NotFound) => TerminalResponse::Done,
                Err(error) => TerminalResponse::Failed(error),
                Ok(terminal) => {
                    self.terminals.lock_clean().remove(id);
                    exit(sink, id, &terminal).await;
                    TerminalResponse::Done
                }
            },
        }
    }

    async fn open(&self, sink: &dyn FrameSink, lease_id: &LeaseId, id: &TerminalId) -> TerminalResponse {
        let terminal = {
            let mut map = self.terminals.lock_clean();
            if let Some(existing) = map.get(id) {
                return if existing.lease_id == *lease_id {
                    TerminalResponse::Opened {
                        terminal_id: id.clone(),
                    }
                } else {
                    TerminalResponse::Failed(TerminalError::IdInUse)
                };
            }
            let open_here = map.values().filter(|terminal| terminal.lease_id == *lease_id).count();
            if open_here >= MAX_TERMINALS_PER_LEASE {
                return TerminalResponse::Failed(TerminalError::TooManyTerminals {
                    max: u32::try_from(MAX_TERMINALS_PER_LEASE).unwrap_or(u32::MAX),
                });
            }
            let terminal = Arc::new(Terminal {
                lease_id: lease_id.clone(),
                seq: tokio::sync::Mutex::new(0),
            });
            map.insert(id.clone(), Arc::clone(&terminal));
            terminal
        };
        emit(sink, id, &terminal, b"$ ").await;
        TerminalResponse::Opened {
            terminal_id: id.clone(),
        }
    }

    /// The terminal `id` if it is open on `lease_id`. Another lease's
    /// terminal reads as `IdInUse`, never as the other lease's data.
    fn owned(&self, lease_id: &LeaseId, id: &TerminalId) -> Result<Arc<Terminal>, TerminalError> {
        match self.terminals.lock_clean().get(id) {
            None => Err(TerminalError::NotFound),
            Some(terminal) if terminal.lease_id == *lease_id => Ok(Arc::clone(terminal)),
            Some(_) => Err(TerminalError::IdInUse),
        }
    }

    /// Ends every terminal of a released lease.
    pub async fn close_lease(&self, sink: &dyn FrameSink, lease_id: &LeaseId) {
        let owned: Vec<(TerminalId, Arc<Terminal>)> = {
            let mut map = self.terminals.lock_clean();
            let ids: Vec<TerminalId> = map
                .iter()
                .filter(|(_, terminal)| terminal.lease_id == *lease_id)
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| map.remove(&id).map(|terminal| (id, terminal)))
                .collect()
        };
        for (id, terminal) in owned {
            exit(sink, &id, &terminal).await;
        }
    }

    /// How many terminals `lease_id` has open.
    pub fn open_count(&self, lease_id: &LeaseId) -> usize {
        self.terminals
            .lock_clean()
            .values()
            .filter(|terminal| terminal.lease_id == *lease_id)
            .count()
    }
}

async fn emit(sink: &dyn FrameSink, id: &TerminalId, terminal: &Terminal, bytes: &[u8]) {
    for chunk in bytes.chunks(MAX_TERMINAL_OUTPUT_CHUNK_BYTES) {
        let mut seq = terminal.seq.lock().await;
        for frame in terminal_data_frame(*seq, chunk).into_iter() {
            *seq += 1;
            publish(sink, id, &frame).await;
        }
    }
}

async fn exit(sink: &dyn FrameSink, id: &TerminalId, terminal: &Terminal) {
    let mut seq = terminal.seq.lock().await;
    let frame = TerminalOutput::Exit {
        seq: *seq,
        exit_code: Some(0),
    };
    *seq += 1;
    publish(sink, id, &frame).await;
}

async fn publish(sink: &dyn FrameSink, id: &TerminalId, frame: &TerminalOutput) {
    let payload = serde_json::to_vec(frame).unwrap_or_default();
    sink.publish(terminal_output(id), Bytes::from(payload)).await;
}
