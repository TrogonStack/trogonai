use async_nats::{PublishError, SubscribeError, UnsubscribeError, client::FlushError};
use trogon_sandbox_protocol::{NatsTransferError, ProviderError};

use crate::ServeDomainError;

/// The worker loop or one request it served failed.
#[derive(Debug, thiserror::Error)]
pub enum KubeVirtWorkerError {
    #[error("subscribing failed: {0}")]
    Subscribe(#[from] SubscribeError),
    #[error("unsubscribing failed: {0}")]
    Unsubscribe(#[from] UnsubscribeError),
    #[error("flushing the connection failed: {0}")]
    Flush(#[from] FlushError),
    #[error("publishing failed: {0}")]
    Publish(#[from] PublishError),
    #[error("a message could not be decoded: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("a file transfer failed: {0}")]
    Transfer(#[from] NatsTransferError),
    #[error("a domain lookup failed: {0}")]
    Domain(#[from] ServeDomainError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
}
