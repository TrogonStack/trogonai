use std::error::Error;
use std::future::{Future, IntoFuture};

use async_nats::HeaderMap;
use async_nats::jetstream::consumer::pull;
use async_nats::jetstream::publish::PublishAck;
use async_nats::jetstream::stream;
use async_nats::subject::ToSubject;
use bytes::Bytes;
use futures::Stream;

pub trait JetStreamContext: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;
    type Stream: Send;

    fn get_or_create_stream<S: Into<stream::Config> + Send>(
        &self,
        config: S,
    ) -> impl Future<Output = Result<Self::Stream, Self::Error>> + Send;
}

pub trait JetStreamPublisher: Send + Sync + Clone + 'static {
    type PublishError: Error + Send + Sync;
    type AckFuture: IntoFuture<Output = Result<PublishAck, Self::PublishError>> + Send;

    fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> impl Future<Output = Result<Self::AckFuture, Self::PublishError>> + Send;
}

pub trait JetStreamGetStream: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;
    type Stream: JetStreamCreateConsumer + Send;

    fn get_stream<T: AsRef<str> + Send>(
        &self,
        stream_name: T,
    ) -> impl Future<Output = Result<Self::Stream, Self::Error>> + Send;
}

pub trait JetStreamCreateConsumer: Send + 'static {
    type Error: Error + Send + Sync;
    type Consumer: JetStreamConsumer;

    fn create_consumer(
        &self,
        config: pull::Config,
    ) -> impl Future<Output = Result<Self::Consumer, Self::Error>> + Send;
}

pub trait JetStreamConsumer: Send + Sync + 'static {
    /// Error from the `messages()` call itself. Maps to async_nats `StreamError`.
    type StreamError: Error + Send + Sync;
    /// Error yielded by individual stream items. Maps to async_nats `MessagesError`.
    type MessagesError: Error + Send + Sync;
    type Message: Send + 'static;
    type Messages: Stream<Item = Result<Self::Message, Self::MessagesError>>
        + Unpin
        + Send
        + 'static;

    fn messages(&self) -> impl Future<Output = Result<Self::Messages, Self::StreamError>> + Send;
}

/// The message type produced by the consumer at the end of the
/// `J → Stream → Consumer → Message` chain.
pub type JsMessageOf<J> =
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as JetStreamConsumer>::Message;
