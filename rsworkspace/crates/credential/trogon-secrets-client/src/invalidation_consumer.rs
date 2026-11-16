use std::future::Future;

use async_nats::jetstream::{
    self,
    consumer::{Consumer, StreamError, pull},
    context::ConsumerInfoError,
};
use futures::Stream;

use super::InvalidationNotice;

/// Reads the notices a consumer delivers, in the order it delivers them.
pub trait InvalidationConsumer: Send + Sync + 'static {
    type Notice: InvalidationNotice;
    type Notices: Stream<Item = Result<Self::Notice, pull::MessagesError>> + Unpin + Send + 'static;

    fn messages(&self) -> impl Future<Output = Result<Self::Notices, StreamError>> + Send;

    /// How many notices the stream already held for this consumer when it
    /// was created, which it delivers before any that arrive after.
    fn backlog(&self) -> u64;

    /// The consumer sequence of the last notice the server delivered to this
    /// consumer, as the server reports it now.
    fn delivered(&self) -> impl Future<Output = Result<u64, ConsumerInfoError>> + Send;
}

impl InvalidationConsumer for Consumer<pull::Config> {
    type Notice = jetstream::Message;
    type Notices = pull::Stream;

    async fn messages(&self) -> Result<Self::Notices, StreamError> {
        Consumer::<pull::Config>::messages(self).await
    }

    fn backlog(&self) -> u64 {
        self.cached_info().num_pending
    }

    async fn delivered(&self) -> Result<u64, ConsumerInfoError> {
        Ok(self.get_info().await?.delivered.consumer_sequence)
    }
}
