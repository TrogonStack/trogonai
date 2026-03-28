use std::time::Duration;

use async_nats::HeaderMap;
use async_nats::jetstream;
use async_nats::jetstream::consumer::pull;
use async_nats::jetstream::publish::PublishAck;
use async_nats::jetstream::stream;
use bytes::Bytes;
use futures::StreamExt;

use super::message::JsMessage;
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

    async fn get_or_create_stream(&self, config: stream::Config) -> Result<(), JetStreamError> {
        self.context
            .get_or_create_stream(config)
            .await
            .map(|_| ())
            .map_err(|e| JetStreamError(e.to_string()))
    }
}

impl JetStreamPublisher for NatsJetStreamClient {
    type PublishError = JetStreamError;

    async fn js_publish_with_headers(
        &self,
        subject: String,
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

impl JsMessage for NatsJsMessage {
    type Error = JetStreamError;

    fn payload(&self) -> &Bytes {
        &self.inner.payload
    }

    fn subject(&self) -> &str {
        self.inner.subject.as_str()
    }

    fn headers(&self) -> Option<&HeaderMap> {
        self.inner.headers.as_ref()
    }

    fn reply(&self) -> Option<&str> {
        self.inner.reply.as_ref().map(|s| s.as_str())
    }

    async fn ack(&self) -> Result<(), JetStreamError> {
        self.inner
            .ack()
            .await
            .map_err(|e| JetStreamError(e.to_string()))
    }

    async fn double_ack(&self) -> Result<(), JetStreamError> {
        self.inner
            .double_ack()
            .await
            .map_err(|e| JetStreamError(e.to_string()))
    }

    async fn nak(&self) -> Result<(), JetStreamError> {
        self.inner
            .ack_with(jetstream::AckKind::Nak(None))
            .await
            .map_err(|e| JetStreamError(e.to_string()))
    }

    async fn nak_with_delay(&self, delay: Duration) -> Result<(), JetStreamError> {
        self.inner
            .ack_with(jetstream::AckKind::Nak(Some(delay)))
            .await
            .map_err(|e| JetStreamError(e.to_string()))
    }

    async fn term(&self) -> Result<(), JetStreamError> {
        self.inner
            .ack_with(jetstream::AckKind::Term)
            .await
            .map_err(|e| JetStreamError(e.to_string()))
    }

    async fn in_progress(&self) -> Result<(), JetStreamError> {
        self.inner
            .ack_with(jetstream::AckKind::Progress)
            .await
            .map_err(|e| JetStreamError(e.to_string()))
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
