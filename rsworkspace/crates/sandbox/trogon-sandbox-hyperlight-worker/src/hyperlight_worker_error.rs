use async_nats::client::FlushError;
use async_nats::jetstream::context::PublishError as JetStreamPublishError;
use async_nats::{PublishError, SubscribeError, UnsubscribeError};

use crate::RuntimeError;

/// Why the worker failed while serving.
#[derive(Debug, thiserror::Error)]
pub enum HyperlightWorkerError {
    /// The provider could not be bound to this worker.
    #[error(transparent)]
    Bind(#[from] RuntimeError),
    /// A subscription could not be opened.
    #[error("cannot subscribe: {0}")]
    Subscribe(#[from] SubscribeError),
    /// A subscription could not be closed.
    #[error("cannot unsubscribe: {0}")]
    Unsubscribe(#[from] UnsubscribeError),
    /// The server did not confirm the subscriptions.
    #[error("cannot flush the connection: {0}")]
    Flush(#[from] FlushError),
    /// A reply could not be published.
    #[error("cannot publish a reply: {0}")]
    Reply(#[from] PublishError),
    /// A lifecycle event could not be published.
    #[error("cannot publish a lifecycle event: {0}")]
    Lifecycle(#[from] JetStreamPublishError),
    /// A message could not be encoded.
    #[error("cannot encode a message: {0}")]
    Encode(#[from] serde_json::Error),
}
