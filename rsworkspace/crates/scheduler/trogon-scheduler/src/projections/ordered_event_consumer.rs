use std::future::Future;

use async_nats::jetstream::{
    self,
    consumer::{Consumer, StreamError, pull},
};
use futures::Stream;

pub(crate) trait OrderedEventConsumer: Send {
    type Messages: Stream<Item = Result<jetstream::Message, pull::OrderedError>> + Unpin + Send;

    fn messages(self) -> impl Future<Output = Result<Self::Messages, StreamError>> + Send;
}

impl OrderedEventConsumer for Consumer<pull::OrderedConfig> {
    type Messages = pull::Ordered;

    async fn messages(self) -> Result<Self::Messages, StreamError> {
        self.messages().await
    }
}
