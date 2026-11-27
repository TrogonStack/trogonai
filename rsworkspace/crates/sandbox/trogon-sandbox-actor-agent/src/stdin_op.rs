use std::sync::Arc;
use std::sync::atomic::AtomicU8;

use tokio::sync::oneshot;
use trogon_sandbox_protocol::StdinWrite;

/// One write queued toward a process's stdin, with the channel its outcome returns on.
#[derive(Debug)]
pub(crate) struct StdinOp {
    pub(crate) write: StdinWrite,
    pub(crate) ack: oneshot::Sender<bool>,
    pub(crate) phase: Arc<AtomicU8>,
}
