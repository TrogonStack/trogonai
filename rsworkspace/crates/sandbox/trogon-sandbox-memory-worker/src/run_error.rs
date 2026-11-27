use async_nats::ConnectError;

use crate::MemoryWorkerError;

/// Why [`run`](crate::run) ended before it was asked to stop.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The NATS server could not be reached.
    #[error("failed to connect to NATS at {url}: {source}")]
    Connect {
        /// The server that was dialed.
        url: String,
        /// Why the connection failed.
        source: ConnectError,
    },
    /// The worker could not start.
    #[error(transparent)]
    Start(#[from] MemoryWorkerError),
}
