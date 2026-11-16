use std::future::Future;

use async_nats::jetstream::{
    self,
    consumer::{Consumer, StreamError, pull},
};
use futures::Stream;

use super::InvalidationNotice;

/// Reads the notices a consumer delivers, in the order it delivers them.
pub trait InvalidationConsumer: Send + 'static {
    type Notice: InvalidationNotice;
    type Notices: Stream<Item = Result<Self::Notice, pull::MessagesError>> + Unpin + Send + 'static;

    fn messages(&self) -> impl Future<Output = Result<Self::Notices, StreamError>> + Send;
}

impl InvalidationConsumer for Consumer<pull::Config> {
    type Notice = jetstream::Message;
    type Notices = pull::Stream;

    async fn messages(&self) -> Result<Self::Notices, StreamError> {
        Consumer::<pull::Config>::messages(self).await
    }
}
