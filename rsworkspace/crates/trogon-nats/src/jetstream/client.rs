use async_nats::HeaderMap;
use async_nats::jetstream;
use async_nats::jetstream::AckKind;
use async_nats::jetstream::consumer::pull;
use async_nats::jetstream::message::OutboundMessage;
use async_nats::jetstream::stream;
use async_nats::subject::ToSubject;
use bytes::Bytes;

use super::message::{JsAck, JsAckWith, JsDoubleAck, JsDoubleAckWith, JsMessageRef};
use super::traits::{
    JetStreamCreateConsumer,
    JetStreamContext, JetStreamCreateKeyValue, JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage,
    JetStreamPublisher,
};

#[derive(Clone)]
pub struct NatsJetStreamClient {
    context: jetstream::Context,
}

impl NatsJetStreamClient {
    pub fn new(context: jetstream::Context) -> Self {
        Self { context }
    }

    pub fn context(&self) -> &jetstream::Context {
        &self.context
    }
}

impl JetStreamContext for NatsJetStreamClient {
    type Error = async_nats::jetstream::context::CreateStreamError;
    type Stream = jetstream::stream::Stream;

    async fn get_or_create_stream<S: Into<stream::Config> + Send>(
        &self,
        config: S,
    ) -> Result<jetstream::stream::Stream, async_nats::jetstream::context::CreateStreamError> {
        self.context.get_or_create_stream(config).await
    }
}

pub type PublishError = async_nats::jetstream::context::PublishError;
pub type PublishAckFuture = async_nats::jetstream::context::PublishAckFuture;

impl JetStreamPublisher for NatsJetStreamClient {
    type PublishError = PublishError;
    type AckFuture = PublishAckFuture;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<Self::AckFuture, Self::PublishError> {
        self.context.publish_with_headers(subject, headers, payload).await
    }
}

impl JetStreamPublishMessage for NatsJetStreamClient {
    type PublishError = PublishError;
    type AckFuture = PublishAckFuture;

    async fn publish_message(&self, message: OutboundMessage) -> Result<Self::AckFuture, Self::PublishError> {
        self.context.publish_message(message).await
    }
}

impl JetStreamCreateKeyValue for NatsJetStreamClient {
    type Store = jetstream::kv::Store;

    async fn create_key_value(
        &self,
        config: jetstream::kv::Config,
    ) -> Result<Self::Store, async_nats::jetstream::context::CreateKeyValueError> {
        self.context.create_key_value(config).await
    }
}

impl JetStreamGetKeyValue for NatsJetStreamClient {
    type Store = jetstream::kv::Store;

    async fn get_key_value<T: Into<String> + Send>(
        &self,
        bucket: T,
    ) -> Result<Self::Store, async_nats::jetstream::context::KeyValueError> {
        self.context.get_key_value(bucket).await
    }
}

impl JsMessageRef for jetstream::Message {
    fn message(&self) -> &async_nats::Message {
        &self.message
    }
}

impl JsAck for jetstream::Message {
    type Error = async_nats::Error;

    async fn ack(&self) -> Result<(), async_nats::Error> {
        self.ack().await
    }
}

impl JsAckWith for jetstream::Message {
    type Error = async_nats::Error;

    async fn ack_with(&self, kind: AckKind) -> Result<(), async_nats::Error> {
        self.ack_with(kind).await
    }
}

impl JsDoubleAck for jetstream::Message {
    type Error = async_nats::Error;

    async fn double_ack(&self) -> Result<(), async_nats::Error> {
        self.double_ack().await
    }
}

impl JsDoubleAckWith for jetstream::Message {
    type Error = async_nats::Error;

    async fn double_ack_with(&self, kind: AckKind) -> Result<(), async_nats::Error> {
        self.double_ack_with(kind).await
    }
}

pub type NatsJetStreamConsumer = jetstream::consumer::Consumer<pull::Config>;

pub type GetStreamError = async_nats::jetstream::context::GetStreamError;
pub type ConsumerError = async_nats::jetstream::stream::ConsumerError;
pub type StreamError = async_nats::jetstream::consumer::StreamError;

impl JetStreamGetStream for NatsJetStreamClient {
    type Error = GetStreamError;
    type Stream = jetstream::stream::Stream;

    async fn get_stream<T: AsRef<str> + Send>(
        &self,
        stream_name: T,
    ) -> Result<jetstream::stream::Stream, GetStreamError> {
        self.context.get_stream(stream_name).await
    }
}
impl JetStreamCreateConsumer for jetstream::stream::Stream {
    type Error = ConsumerError;
    type Consumer = NatsJetStreamConsumer;

    async fn create_consumer(&self, config: pull::Config) -> Result<NatsJetStreamConsumer, ConsumerError> {
        self.create_consumer(config).await
    }
}
pub type MessagesError = async_nats::jetstream::consumer::pull::MessagesError;
