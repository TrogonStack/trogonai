use async_nats::SubscribeError;
use trogon_sandbox_protocol::AuthorityModeError;

use crate::retire_error::RetireError;
use crate::worker_binding_error::WorkerBindingError;

/// Why the worker loop could not start or could not stop cleanly.
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// The provider already serves another worker.
    #[error(transparent)]
    Bind(#[from] WorkerBindingError),
    /// The authority mode named in the environment is not valid.
    #[error(transparent)]
    Authority(#[from] AuthorityModeError),
    /// A dispatch, cancel or verb subscription could not be created.
    #[error("failed to subscribe: {0}")]
    Subscribe(#[from] SubscribeError),
    /// A guest could not be stopped while shutting down.
    #[error(transparent)]
    Shutdown(#[from] RetireError),
}
