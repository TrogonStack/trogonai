use std::sync::Arc;
use std::sync::atomic::AtomicU8;

use tokio::sync::oneshot;
use trogon_sandbox_protocol::StdinWrite;

/// Where a queued write stands, shared between the caller waiting on it and the task that performs it.
pub(crate) const PHASE_QUEUED: u8 = 0;
pub(crate) const PHASE_STARTED: u8 = 1;
pub(crate) const PHASE_CANCELLED: u8 = 2;

/// One write queued toward a process's stdin, with the channel its outcome returns on.
#[derive(Debug)]
pub(crate) struct StdinOp {
    pub(crate) write: StdinWrite,
    pub(crate) ack: oneshot::Sender<bool>,
    pub(crate) phase: Arc<AtomicU8>,
}
