use async_nats::{PublishError, SubscribeError, UnsubscribeError, client::FlushError};
use trogon_sandbox_protocol::{DispatchDecodeError, NatsTransferError};

use crate::ModalProviderError;

/// The worker loop or one request it served failed.
#[derive(Debug, thiserror::Error)]
pub enum ModalWorkerError {
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
    #[error("a dispatch was refused: {0}")]
    Dispatch(#[from] DispatchDecodeError),
    #[error("a file transfer failed: {0}")]
    Transfer(#[from] NatsTransferError),
    #[error(transparent)]
    Provider(#[from] ModalProviderError),
}
