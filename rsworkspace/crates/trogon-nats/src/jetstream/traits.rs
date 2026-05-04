use async_nats::HeaderMap;
use async_nats::jetstream::consumer::pull;
use async_nats::jetstream::context;
use async_nats::jetstream::kv;
use async_nats::jetstream::message::OutboundMessage;
use async_nats::jetstream::publish::PublishAck;
use async_nats::jetstream::stream;
use async_nats::jetstream::{self};
use async_nats::subject::ToSubject;
use bytes::Bytes;
use futures::Stream;
use std::error::Error;
use std::future::{Future, IntoFuture};

pub trait JetStreamContext: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;
    type Stream: Send;

    fn get_or_create_stream<S: Into<stream::Config> + Send>(
        &self,
        config: S,
    ) -> impl Future<Output = Result<Self::Stream, Self::Error>> + Send;
}

pub trait JetStreamKeyValueStatus: Send + Sync + Clone + 'static {
    fn status(&self)
    -> impl Future<Output = Result<async_nats::jetstream::kv::bucket::Status, kv::StatusError>> + Send;
}

pub trait JetStreamKeyValueCreateWithTtl: Send + Sync + Clone + 'static {
    fn create_with_ttl(
        &self,
        key: &str,
        value: Bytes,
        ttl: std::time::Duration,
    ) -> impl Future<Output = Result<u64, kv::CreateError>> + Send;
}

pub trait JetStreamKeyValueUpdate: Send + Sync + Clone + 'static {
    fn update(
        &self,
        key: &str,
        value: Bytes,
        revision: u64,
    ) -> impl Future<Output = Result<u64, kv::UpdateError>> + Send;
}

pub trait JetStreamKeyValueDeleteExpectRevision: Send + Sync + Clone + 'static {
    fn delete_expect_revision(
        &self,
        key: &str,
        revision: Option<u64>,
    ) -> impl Future<Output = Result<(), kv::DeleteError>> + Send;
}

pub trait JetStreamCreateKeyValue: Send + Sync + Clone + 'static {
    type Store: JetStreamKeyValueStatus
        + JetStreamKeyValueCreateWithTtl
        + JetStreamKeyValueUpdate
        + JetStreamKeyValueDeleteExpectRevision
        + Clone
        + Send
        + Sync
        + 'static;

    fn create_key_value(
        &self,
        config: kv::Config,
    ) -> impl Future<Output = Result<Self::Store, context::CreateKeyValueError>> + Send;
}

pub trait JetStreamGetKeyValue: Send + Sync + Clone + 'static {
    type Store: JetStreamKeyValueStatus
        + JetStreamKeyValueCreateWithTtl
        + JetStreamKeyValueUpdate
        + JetStreamKeyValueDeleteExpectRevision
        + Clone
        + Send
        + Sync
        + 'static;

    fn get_key_value<T: Into<String> + Send>(
        &self,
        bucket: T,
    ) -> impl Future<Output = Result<Self::Store, context::KeyValueError>> + Send;
}

pub trait JetStreamPublisher: Send + Sync + Clone + 'static {
    type PublishError: Error + Send + Sync;
    type AckFuture: IntoFuture<Output = Result<PublishAck, Self::PublishError>, IntoFuture: Send> + Send;

    fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> impl Future<Output = Result<Self::AckFuture, Self::PublishError>> + Send;
}

pub trait JetStreamPublishMessage: Send + Sync + Clone + 'static {
    type PublishError: Error + Send + Sync;
    type AckFuture: IntoFuture<Output = Result<PublishAck, Self::PublishError>, IntoFuture: Send> + Send;

    fn publish_message(
        &self,
        message: OutboundMessage,
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

    fn create_consumer(&self, config: pull::Config)
    -> impl Future<Output = Result<Self::Consumer, Self::Error>> + Send;
}

pub trait JetStreamConsumer: Send + Sync + 'static {
    /// Error from the `messages()` call itself. Maps to async_nats `StreamError`.
    type StreamError: Error + Send + Sync;
    /// Error yielded by individual stream items. Maps to async_nats `MessagesError`.
    type MessagesError: Error + Send + Sync;
    type Message: Send + 'static;
    type Messages: Stream<Item = Result<Self::Message, Self::MessagesError>> + Unpin + Send + 'static;

    fn messages(&self) -> impl Future<Output = Result<Self::Messages, Self::StreamError>> + Send;
}

/// The message type produced by the consumer at the end of the
/// `J → Stream → Consumer → Message` chain.
pub type JsMessageOf<J> =
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as JetStreamConsumer>::Message;

impl JetStreamKeyValueStatus for jetstream::kv::Store {
    fn status(
        &self,
    ) -> impl Future<Output = Result<async_nats::jetstream::kv::bucket::Status, kv::StatusError>> + Send {
        jetstream::kv::Store::status(self)
    }
}

impl JetStreamKeyValueCreateWithTtl for jetstream::kv::Store {
    async fn create_with_ttl(&self, key: &str, value: Bytes, ttl: std::time::Duration) -> Result<u64, kv::CreateError> {
        self.create_with_ttl(key, value, ttl).await
    }
}

impl JetStreamKeyValueUpdate for jetstream::kv::Store {
    async fn update(&self, key: &str, value: Bytes, revision: u64) -> Result<u64, kv::UpdateError> {
        self.update(key, value, revision).await
    }
}

impl JetStreamKeyValueDeleteExpectRevision for jetstream::kv::Store {
    async fn delete_expect_revision(&self, key: &str, revision: Option<u64>) -> Result<(), kv::DeleteError> {
        self.delete_expect_revision(key, revision).await
    }
}

impl JetStreamCreateKeyValue for jetstream::Context {
    type Store = kv::Store;

    fn create_key_value(
        &self,
        config: kv::Config,
    ) -> impl Future<Output = Result<Self::Store, context::CreateKeyValueError>> + Send {
        jetstream::Context::create_key_value(self, config)
    }
}

impl JetStreamGetKeyValue for jetstream::Context {
    type Store = kv::Store;

    fn get_key_value<T: Into<String> + Send>(
        &self,
        bucket: T,
    ) -> impl Future<Output = Result<Self::Store, context::KeyValueError>> + Send {
        jetstream::Context::get_key_value(self, bucket)
    }
}

impl JetStreamPublisher for jetstream::Context {
    type PublishError = context::PublishError;
    type AckFuture = context::PublishAckFuture;

    fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> impl Future<Output = Result<Self::AckFuture, Self::PublishError>> + Send {
        jetstream::Context::publish_with_headers(self, subject, headers, payload)
    }
}

impl JetStreamPublishMessage for jetstream::Context {
    type PublishError = context::PublishError;
    type AckFuture = context::PublishAckFuture;

    fn publish_message(
        &self,
        message: OutboundMessage,
    ) -> impl Future<Output = Result<Self::AckFuture, Self::PublishError>> + Send {
        context::traits::Publisher::publish_message(self, message)
    }
}

#[cfg(not(coverage))]
impl JetStreamGetStream for jetstream::Context {
    type Error = context::GetStreamError;
    type Stream = stream::Stream;

    fn get_stream<T: AsRef<str> + Send>(
        &self,
        stream_name: T,
    ) -> impl Future<Output = Result<Self::Stream, Self::Error>> + Send {
        jetstream::Context::get_stream(self, stream_name)
    }
}

impl JetStreamCreateConsumer for jetstream::stream::Stream {
    type Error = async_nats::jetstream::stream::ConsumerError;
    type Consumer = jetstream::consumer::Consumer<pull::Config>;

    async fn create_consumer(&self, config: pull::Config) -> Result<Self::Consumer, Self::Error> {
        self.create_consumer(config).await
    }
}

impl JetStreamConsumer for jetstream::consumer::Consumer<pull::Config> {
    type StreamError = async_nats::jetstream::consumer::StreamError;
    type MessagesError = async_nats::jetstream::consumer::pull::MessagesError;
    type Message = jetstream::Message;
    type Messages = async_nats::jetstream::consumer::pull::Stream;

    async fn messages(&self) -> Result<Self::Messages, Self::StreamError> {
        self.messages().await
    }
}
