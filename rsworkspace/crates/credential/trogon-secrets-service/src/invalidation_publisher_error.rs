use trogon_nats::jetstream::{JetStreamConsumer, JetStreamCreateConsumer};

use super::constants::INVALIDATION_PUBLISHER_DURABLE;

/// The [`InvalidationPublisherError`] of a publisher reading credential events
/// through `S`.
pub(crate) type PublisherError<S> = InvalidationPublisherError<
    <S as JetStreamCreateConsumer>::Error,
    <<S as JetStreamCreateConsumer>::Consumer as JetStreamConsumer>::StreamError,
>;

/// Why the invalidation publisher stopped reading the credential stream before
/// it was told to.
///
/// Holders cache a release only because a notice tells them when to drop it
/// (ADR#0074), so a publisher that stops leaves the service unable to keep
/// that promise; the service stops with it rather than serve on.
#[derive(Debug, thiserror::Error)]
pub enum InvalidationPublisherError<C, M> {
    #[error("durable consumer '{INVALIDATION_PUBLISHER_DURABLE}' could not be created on the credential stream")]
    Consumer(#[source] C),
    #[error("durable consumer '{INVALIDATION_PUBLISHER_DURABLE}' could not start delivering credential events")]
    Messages(#[source] M),
    #[error("durable consumer '{INVALIDATION_PUBLISHER_DURABLE}' stopped delivering credential events before shutdown")]
    MessagesEnded,
}

#[cfg(test)]
mod tests;
