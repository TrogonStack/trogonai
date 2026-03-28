use std::error::Error;
use std::future::Future;

use async_nats::HeaderMap;
use async_nats::jetstream::consumer::pull;
use async_nats::jetstream::publish::PublishAck;
use async_nats::jetstream::stream;
use bytes::Bytes;
use futures::Stream;

use super::message::JsMessage;

pub trait JetStreamContext: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;

    fn get_or_create_stream(
        &self,
        config: stream::Config,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

pub trait JetStreamPublisher: Send + Sync + Clone + 'static {
    type PublishError: Error + Send + Sync;

    fn js_publish_with_headers(
        &self,
        subject: String,
        headers: HeaderMap,
        payload: Bytes,
    ) -> impl Future<Output = Result<PublishAck, Self::PublishError>> + Send;
}

pub trait JetStreamConsumerFactory: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;
    type Consumer: JetStreamConsumer;

    fn create_consumer(
        &self,
        stream_name: &str,
        config: pull::Config,
    ) -> impl Future<Output = Result<Self::Consumer, Self::Error>> + Send;
}

pub trait JetStreamConsumer: Send + Sync + 'static {
    type Error: Error + Send + Sync;
    type Messages: Stream<Item = Result<JsMessage, Self::Error>> + Unpin + Send + 'static;

    fn messages(&self) -> impl Future<Output = Result<Self::Messages, Self::Error>> + Send;
}
