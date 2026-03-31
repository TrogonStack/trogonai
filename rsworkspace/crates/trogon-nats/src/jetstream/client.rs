use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::task::{Context, Poll};

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
    type Stream = jetstream::stream::Stream;

    async fn get_or_create_stream<S: Into<stream::Config> + Send>(
        &self,
        config: S,
    ) -> Result<jetstream::stream::Stream, JetStreamError> {
        self.context
            .get_or_create_stream(config)
            .await
            .map_err(|e| JetStreamError(e.to_string()))
    }
}

/// Wraps [`async_nats::jetstream::context::PublishAckFuture`] to map its error
/// to [`JetStreamError`].
pub struct JsPublishAckFuture {
    inner: Pin<
        Box<
            dyn Future<
                    Output = Result<
                        PublishAck,
                        async_nats::error::Error<async_nats::jetstream::context::PublishErrorKind>,
                    >,
                > + Send,
        >,
    >,
}

impl Future for JsPublishAckFuture {
    type Output = Result<PublishAck, JetStreamError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner
            .as_mut()
            .poll(cx)
            .map(|r| r.map_err(|e| JetStreamError(e.to_string())))
    }
}

impl JetStreamPublisher for NatsJetStreamClient {
    type PublishError = JetStreamError;
    type AckFuture = JsPublishAckFuture;

    async fn js_publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<JsPublishAckFuture, JetStreamError> {
        let ack_future = self
            .context
            .publish_with_headers(subject, headers, payload)
            .await
            .map_err(|e| JetStreamError(e.to_string()))?;
        Ok(JsPublishAckFuture {
            inner: ack_future.into_future(),
        })
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
