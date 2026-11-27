use std::sync::Arc;
use std::sync::atomic::AtomicU8;

use tokio::sync::oneshot;
use trogon_sandbox_protocol::StdinWrite;

/// One queued stdin write and the channel its outcome is acknowledged on.
pub(crate) struct StdinOp {
    pub(crate) write: StdinWrite,
    pub(crate) ack: oneshot::Sender<bool>,
    pub(crate) phase: Arc<AtomicU8>,
}
