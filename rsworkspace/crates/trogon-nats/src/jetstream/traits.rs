use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

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
    type AckFuture: Future<Output = Result<PublishAck, Self::PublishError>> + Send;

    fn js_publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> impl Future<Output = Result<Self::AckFuture, Self::PublishError>> + Send;
}

/// Immediately-ready ack future for mocks and no-op impls.
pub struct ReadyAckFuture<E> {
    result: Option<Result<PublishAck, E>>,
}

impl<E> ReadyAckFuture<E> {
    pub fn new(result: Result<PublishAck, E>) -> Self {
        Self {
            result: Some(result),
        }
    }
}

impl<E: Unpin> Future for ReadyAckFuture<E> {
    type Output = Result<PublishAck, E>;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(
            self.result
                .take()
                .expect("ReadyAckFuture polled after completion"),
        )
    }
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
    type Message: Send + 'static;
    type Messages: Stream<Item = Result<Self::Message, Self::Error>> + Unpin + Send + 'static;

    fn messages(&self) -> impl Future<Output = Result<Self::Messages, Self::Error>> + Send;
}

/// No-op error for the `()` JetStream impls. These impls exist only to satisfy
/// trait bounds when `Bridge<N, C, ()>` is used without JetStream — the methods
/// are never called because `bridge.js()` returns `None`.
#[derive(Debug)]
pub struct NoJetStream;

impl std::fmt::Display for NoJetStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JetStream not configured")
    }
}

impl Error for NoJetStream {}

impl JetStreamPublisher for () {
    type PublishError = NoJetStream;
    type AckFuture = ReadyAckFuture<NoJetStream>;

    async fn js_publish_with_headers<S: ToSubject + Send>(
        &self,
        _subject: S,
        _headers: HeaderMap,
        _payload: Bytes,
    ) -> Result<Self::AckFuture, NoJetStream> {
        Err(NoJetStream)
    }
}

impl JetStreamConsumerFactory for () {
    type Error = NoJetStream;
    type Consumer = NoOpConsumer;

    async fn create_consumer(
        &self,
        _stream_name: &str,
        _config: pull::Config,
    ) -> Result<NoOpConsumer, NoJetStream> {
        Err(NoJetStream)
    }
}

/// Stub consumer for `J = ()`. Never produced at runtime because
/// `()::create_consumer()` always returns `Err`.
pub struct NoOpConsumer;

impl JetStreamConsumer for NoOpConsumer {
    type Error = NoJetStream;
    type Message = NoOpMessage;
    type Messages = futures::stream::Empty<Result<NoOpMessage, NoJetStream>>;

    async fn messages(&self) -> Result<Self::Messages, NoJetStream> {
        Err(NoJetStream)
    }
}

/// Stub message for [`NoOpConsumer`]. Never produced at runtime because
/// `NoOpConsumer::messages()` always returns `Err`. Exists only to satisfy
/// generic bounds when `J = ()`.
#[derive(Debug)]
pub struct NoOpMessage {
    inner: async_nats::Message,
}

impl NoOpMessage {
    #[cfg(test)]
    fn stub() -> Self {
        Self {
            inner: async_nats::Message {
                subject: "".into(),
                reply: None,
                payload: bytes::Bytes::new(),
                headers: None,
                status: None,
                description: None,
                length: 0,
            },
        }
    }
}

impl super::message::JsMessageRef for NoOpMessage {
    fn message(&self) -> &async_nats::Message {
        &self.inner
    }
}

impl super::message::JsAck for NoOpMessage {
    type Error = NoJetStream;

    async fn ack(&self) -> Result<(), NoJetStream> {
        Err(NoJetStream)
    }
}

impl super::message::JsAckWith for NoOpMessage {
    type Error = NoJetStream;

    async fn ack_with(&self, _kind: async_nats::jetstream::AckKind) -> Result<(), NoJetStream> {
        Err(NoJetStream)
    }
}

impl super::message::JsDoubleAck for NoOpMessage {
    type Error = NoJetStream;

    async fn double_ack(&self) -> Result<(), NoJetStream> {
        Err(NoJetStream)
    }
}

impl super::message::JsDoubleAckWith for NoOpMessage {
    type Error = NoJetStream;

    async fn double_ack_with(
        &self,
        _kind: async_nats::jetstream::AckKind,
    ) -> Result<(), NoJetStream> {
        Err(NoJetStream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_jetstream_display() {
        let err = NoJetStream;
        assert_eq!(err.to_string(), "JetStream not configured");
    }

    #[test]
    fn no_jetstream_is_error() {
        let err: &dyn Error = &NoJetStream;
        assert!(err.source().is_none());
    }

    #[tokio::test]
    async fn no_op_consumer_messages_returns_err() {
        let consumer = NoOpConsumer;
        let result = JetStreamConsumer::messages(&consumer).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "JetStream not configured");
    }

    #[tokio::test]
    async fn unit_publisher_returns_err() {
        let result = ().js_publish_with_headers("s", HeaderMap::new(), Bytes::new()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn unit_consumer_factory_returns_err() {
        let result = ().create_consumer("s", pull::Config::default()).await;
        assert!(result.is_err());
    }

    #[test]
    fn no_op_message_ref() {
        use super::super::message::JsMessageRef;
        let msg = NoOpMessage::stub();
        let inner = msg.message();
        assert!(inner.payload.is_empty());
        assert_eq!(inner.subject.as_str(), "");
        assert!(inner.headers.is_none());
        assert!(inner.reply.is_none());
    }

    #[tokio::test]
    async fn no_op_message_signals() {
        use super::super::message::{JsAck, JsAckWith, JsDoubleAck, JsDoubleAckWith};
        let msg = NoOpMessage::stub();
        assert!(msg.ack().await.is_err());
        assert!(
            msg.ack_with(async_nats::jetstream::AckKind::Ack)
                .await
                .is_err()
        );
        assert!(msg.double_ack().await.is_err());
        assert!(
            msg.double_ack_with(async_nats::jetstream::AckKind::Ack)
                .await
                .is_err()
        );
    }
}
