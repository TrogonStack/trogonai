use async_nats::SubscribeError;
use trogon_sandbox_protocol::AuthorityModeError;

/// Why the worker loop could not start.
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// The authority mode named in the environment is not valid.
    #[error(transparent)]
    Authority(#[from] AuthorityModeError),
    /// A dispatch or verb subscription could not be created.
    #[error("failed to subscribe: {0}")]
    Subscribe(#[from] SubscribeError),
}
