use std::future::Future;

use async_nats::jetstream::{
    consumer::{Consumer, pull},
    stream,
};
use trogon_nats::jetstream::JetStreamGetStreamInfo;

use super::ordered_event_consumer::OrderedEventConsumer;

pub(crate) trait OrderedEventStream: JetStreamGetStreamInfo {
    type Consumer: OrderedEventConsumer;

    fn create_ordered_consumer(
        &self,
        config: pull::OrderedConfig,
    ) -> impl Future<Output = Result<Self::Consumer, stream::ConsumerError>> + Send;
}

impl OrderedEventStream for stream::Stream {
    type Consumer = Consumer<pull::OrderedConfig>;

    async fn create_ordered_consumer(
        &self,
        config: pull::OrderedConfig,
    ) -> Result<Self::Consumer, stream::ConsumerError> {
        self.create_consumer(config).await
    }
}
