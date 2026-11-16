use std::future::Future;

use async_nats::jetstream::consumer::{Consumer, pull};
use async_nats::jetstream::stream;

use super::InvalidationConsumer;

/// Creates a client's own consumer on the invalidation stream.
pub trait InvalidationStream: Send + Sync + 'static {
    type Consumer: InvalidationConsumer;

    /// The configuration the stream had when it was looked up.
    fn config(&self) -> stream::Config;

    fn create_consumer(
        &self,
        config: pull::Config,
    ) -> impl Future<Output = Result<Self::Consumer, stream::ConsumerError>> + Send;
}

impl InvalidationStream for stream::Stream {
    type Consumer = Consumer<pull::Config>;

    fn config(&self) -> stream::Config {
        self.cached_info().config.clone()
    }

    async fn create_consumer(&self, config: pull::Config) -> Result<Self::Consumer, stream::ConsumerError> {
        stream::Stream::create_consumer(self, config).await
    }
}
