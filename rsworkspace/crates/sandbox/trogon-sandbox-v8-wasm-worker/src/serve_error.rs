use async_nats::ConnectError;
use trogon_sandbox_contract::TokenError;

use crate::WorkerError;

/// Why the worker process could not serve.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The NATS server could not be reached.
    #[error("failed to connect to NATS at {url}: {source}")]
    Connect {
        url: String,
        #[source]
        source: ConnectError,
    },
    /// The generated worker id was rejected.
    #[error(transparent)]
    WorkerId(#[from] TokenError),
    /// The worker loop failed.
    #[error(transparent)]
    Worker(#[from] WorkerError),
}
