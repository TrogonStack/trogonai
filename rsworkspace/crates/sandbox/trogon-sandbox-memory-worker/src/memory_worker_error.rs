use async_nats::SubscribeError;
use async_nats::client::FlushError;
use trogon_sandbox_contract::TokenError;
use trogon_sandbox_protocol::AuthorityModeError;

/// Why a [`MemoryWorker`](crate::MemoryWorker) could not start.
#[derive(Debug, thiserror::Error)]
pub enum MemoryWorkerError {
    /// The generated worker id is not a valid worker id.
    #[error("invalid worker id: {0}")]
    WorkerId(#[from] TokenError),
    /// The authority mode in the environment is not recognized.
    #[error(transparent)]
    Authority(#[from] AuthorityModeError),
    /// A provider subject could not be subscribed.
    #[error("failed to subscribe to the provider subjects: {0}")]
    Subscribe(#[from] SubscribeError),
    /// The subscriptions could not be flushed to the server.
    #[error("failed to flush the subscriptions: {0}")]
    Flush(#[from] FlushError),
}
