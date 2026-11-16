use async_nats::jetstream::consumer::StreamError;
use async_nats::jetstream::context::GetStreamError;
use async_nats::jetstream::stream::ConsumerError;

use super::InvalidationStreamError;

/// Why a cached client could not start reading release invalidations, and so
/// refused to start: a client that holds releases it never hears revoked would
/// keep serving them.
#[derive(Debug, thiserror::Error)]
pub enum InvalidationStartError {
    #[error("the invalidation stream could not be found")]
    Stream(#[from] GetStreamError),
    /// The stream could remove or rewrite a notice before a reader starting
    /// [`crate::INVALIDATION_LOOKBACK`] back reads it.
    #[error(transparent)]
    StreamConfig(#[from] InvalidationStreamError),
    #[error("the invalidation consumer could not be created")]
    Consumer(#[from] ConsumerError),
    #[error("the invalidation consumer could not be read")]
    Messages(#[from] StreamError),
}
