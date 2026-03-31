use async_nats::HeaderMap;
use async_nats::jetstream;
use async_nats::jetstream::AckKind;
use async_nats::jetstream::consumer::pull;
use async_nats::jetstream::publish::PublishAck;
use async_nats::jetstream::stream;
use async_nats::subject::ToSubject;
use bytes::Bytes;
use futures::StreamExt;

use super::message::{JsAck, JsAckWith, JsDoubleAck, JsDoubleAckWith, JsMessageRef};
use super::traits::{
    JetStreamConsumer, JetStreamConsumerFactory, JetStreamContext, JetStreamPublisher,
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

#[derive(Debug)]
pub struct JetStreamError(pub String);

impl std::fmt::Display for JetStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for JetStreamError {}

impl JetStreamContext for NatsJetStreamClient {
    type Error = JetStreamError;

    async fn get_or_create_stream<S: Into<stream::Config> + Send>(
        &self,
        config: S,
    ) -> Result<(), JetStreamError> {
        self.context
            .get_or_create_stream(config)
            .await
            .map(|_| ())
            .map_err(|e| JetStreamError(e.to_string()))
    }
}

impl JetStreamPublisher for NatsJetStreamClient {
    type PublishError = JetStreamError;

    async fn js_publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<PublishAck, JetStreamError> {
        self.context
            .publish_with_headers(subject, headers, payload)
            .await
            .map_err(|e| JetStreamError(e.to_string()))?
            .await
            .map_err(|e| JetStreamError(e.to_string()))
    }
}

pub struct NatsJsMessage {
    inner: jetstream::Message,
}

impl NatsJsMessage {
    pub fn new(inner: jetstream::Message) -> Self {
        Self { inner }
    }
}

impl JsMessageRef for NatsJsMessage {
    fn message(&self) -> &async_nats::Message {
        &self.inner.message
    }
}

impl JsAck for NatsJsMessage {
    type Error = async_nats::Error;

    async fn ack(&self) -> Result<(), async_nats::Error> {
        self.inner.ack().await
    }
}

impl JsAckWith for NatsJsMessage {
    type Error = async_nats::Error;

    async fn ack_with(&self, kind: AckKind) -> Result<(), async_nats::Error> {
        self.inner.ack_with(kind).await
    }
}

impl JsDoubleAck for NatsJsMessage {
    type Error = async_nats::Error;

    async fn double_ack(&self) -> Result<(), async_nats::Error> {
        self.inner.double_ack().await
    }
}

impl JsDoubleAckWith for NatsJsMessage {
    type Error = async_nats::Error;

    async fn double_ack_with(&self, kind: AckKind) -> Result<(), async_nats::Error> {
        self.inner.double_ack_with(kind).await
    }
}

pub struct NatsJetStreamConsumer {
    inner: jetstream::consumer::Consumer<pull::Config>,
}

impl NatsJetStreamConsumer {
    pub fn new(inner: jetstream::consumer::Consumer<pull::Config>) -> Self {
        Self { inner }
    }
}

impl JetStreamConsumerFactory for NatsJetStreamClient {
    type Error = JetStreamError;
    type Consumer = NatsJetStreamConsumer;

    async fn create_consumer(
        &self,
        stream_name: &str,
        config: pull::Config,
    ) -> Result<NatsJetStreamConsumer, JetStreamError> {
        let stream = self
            .context
            .get_stream(stream_name)
            .await
            .map_err(|e| JetStreamError(e.to_string()))?;

        let consumer = stream
            .create_consumer(config)
            .await
            .map_err(|e| JetStreamError(e.to_string()))?;

        Ok(NatsJetStreamConsumer::new(consumer))
    }
}

impl JetStreamConsumer for NatsJetStreamConsumer {
    type Error = JetStreamError;
    type Message = NatsJsMessage;
    type Messages = futures::stream::BoxStream<'static, Result<NatsJsMessage, JetStreamError>>;

    async fn messages(&self) -> Result<Self::Messages, JetStreamError> {
        let messages = self
            .inner
            .messages()
            .await
            .map_err(|e| JetStreamError(e.to_string()))?;

        Ok(messages
            .map(|result| {
                result
                    .map(NatsJsMessage::new)
                    .map_err(|e| JetStreamError(e.to_string()))
            })
            .boxed())
    }
}
