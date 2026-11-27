use async_nats::SubscribeError;
use trogon_sandbox_protocol::AuthorityModeError;

use crate::WorkerBindingError;

/// Why the worker loop could not start.
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// The provider already serves another worker.
    #[error(transparent)]
    Bind(#[from] WorkerBindingError),
    /// The authority mode named in the environment is not valid.
    #[error(transparent)]
    Authority(#[from] AuthorityModeError),
    /// A dispatch or cancel subscription could not be created.
    #[error("failed to subscribe: {0}")]
    Subscribe(#[from] SubscribeError),
}
