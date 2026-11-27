use std::io;

use tokio::sync::AcquireError;

/// Why the accept loop stopped.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("accept tcp: {0}")]
    Accept(#[from] io::Error),
    #[error("connection slot: {0}")]
    Slot(#[from] AcquireError),
}
