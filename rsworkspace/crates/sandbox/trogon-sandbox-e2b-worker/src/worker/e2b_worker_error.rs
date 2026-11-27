use async_nats::PublishError;
use async_nats::SubscribeError;
use async_nats::client::FlushError;
use trogon_sandbox_protocol::NatsTransferError;

use crate::{E2bDispatchError, E2bProviderError};

/// The worker could not start, or one request it served failed.
#[derive(Debug, thiserror::Error)]
pub enum E2bWorkerError {
    #[error("subscribing failed: {0}")]
    Subscribe(#[from] SubscribeError),
    #[error("publishing failed: {0}")]
    Publish(#[from] PublishError),
    #[error("flushing the connection failed: {0}")]
    Flush(#[from] FlushError),
    #[error("a message could not be encoded: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("a file transfer failed: {0}")]
    Transfer(#[from] NatsTransferError),
    /// A request body that does not decode.
    #[error("{what}: {source}")]
    Decode {
        what: &'static str,
        #[source]
        source: serde_json::Error,
    },
    /// The E2B provider refused to bind to this worker or to prepare its snapshots.
    #[error(transparent)]
    Provider(#[from] E2bProviderError),
    /// Targeted dispatch could not be initialized.
    #[error(transparent)]
    Dispatch(#[from] E2bDispatchError),
}

#[cfg(test)]
mod tests;
