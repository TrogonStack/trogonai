use std::error::Error;
use std::future::Future;
use std::time::Duration;

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
    type Message: JsMessage;
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

    async fn js_publish_with_headers(
        &self,
        _subject: String,
        _headers: HeaderMap,
        _payload: Bytes,
    ) -> Result<PublishAck, NoJetStream> {
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

pub struct NoOpConsumer;

impl JetStreamConsumer for NoOpConsumer {
    type Error = NoJetStream;
    type Message = NoOpMessage;
    type Messages = futures::stream::Empty<Result<NoOpMessage, NoJetStream>>;

    async fn messages(&self) -> Result<Self::Messages, NoJetStream> {
        Err(NoJetStream)
    }
}

/// Placeholder message type for [`NoOpConsumer`]. Never instantiated because
/// `NoOpConsumer::messages()` always returns `Err`. The accessor methods panic
/// because `NoOpMessage` is never produced — the async signal methods return
/// `Err(NoJetStream)` for testability.
pub struct NoOpMessage;

impl JsMessage for NoOpMessage {
    type Error = NoJetStream;

    fn payload(&self) -> &bytes::Bytes {
        unreachable!("NoOpMessage is never instantiated")
    }
    fn subject(&self) -> &str {
        unreachable!("NoOpMessage is never instantiated")
    }
    fn headers(&self) -> Option<&async_nats::HeaderMap> {
        unreachable!("NoOpMessage is never instantiated")
    }
    fn reply(&self) -> Option<&str> {
        unreachable!("NoOpMessage is never instantiated")
    }
    async fn ack(&self) -> Result<(), NoJetStream> {
        Err(NoJetStream)
    }
    async fn double_ack(&self) -> Result<(), NoJetStream> {
        Err(NoJetStream)
    }
    async fn nak(&self) -> Result<(), NoJetStream> {
        Err(NoJetStream)
    }
    async fn nak_with_delay(&self, _delay: Duration) -> Result<(), NoJetStream> {
        Err(NoJetStream)
    }
    async fn term(&self) -> Result<(), NoJetStream> {
        Err(NoJetStream)
    }
    async fn in_progress(&self) -> Result<(), NoJetStream> {
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
        let err = result.err().unwrap();
        assert_eq!(err.to_string(), "JetStream not configured");
    }

    #[test]
    fn no_op_message_type_compiles() {
        fn _assert_js_message<T: JsMessage>() {}
        _assert_js_message::<NoOpMessage>();
    }

    #[tokio::test]
    async fn unit_publisher_returns_err() {
        let result = ().js_publish_with_headers("s".into(), HeaderMap::new(), Bytes::new()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn unit_consumer_factory_returns_err() {
        let result = ().create_consumer("s", pull::Config::default()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn no_op_message_ack_returns_err() {
        let msg = NoOpMessage;
        assert!(msg.ack().await.is_err());
    }

    #[tokio::test]
    async fn no_op_message_term_returns_err() {
        let msg = NoOpMessage;
        assert!(msg.term().await.is_err());
    }

    #[tokio::test]
    async fn no_op_message_nak_returns_err() {
        let msg = NoOpMessage;
        assert!(msg.nak().await.is_err());
    }

    #[tokio::test]
    async fn no_op_message_double_ack_returns_err() {
        let msg = NoOpMessage;
        assert!(msg.double_ack().await.is_err());
    }

    #[tokio::test]
    async fn no_op_message_nak_with_delay_returns_err() {
        let msg = NoOpMessage;
        assert!(msg.nak_with_delay(Duration::from_secs(1)).await.is_err());
    }

    #[tokio::test]
    async fn no_op_message_in_progress_returns_err() {
        let msg = NoOpMessage;
        assert!(msg.in_progress().await.is_err());
    }

    #[test]
    #[should_panic(expected = "NoOpMessage is never instantiated")]
    fn no_op_message_payload_panics() {
        let msg = NoOpMessage;
        let _ = msg.payload();
    }

    #[test]
    #[should_panic(expected = "NoOpMessage is never instantiated")]
    fn no_op_message_subject_panics() {
        let msg = NoOpMessage;
        let _ = msg.subject();
    }

    #[test]
    #[should_panic(expected = "NoOpMessage is never instantiated")]
    fn no_op_message_headers_panics() {
        let msg = NoOpMessage;
        let _ = msg.headers();
    }

    #[test]
    #[should_panic(expected = "NoOpMessage is never instantiated")]
    fn no_op_message_reply_panics() {
        let msg = NoOpMessage;
        let _ = msg.reply();
    }
}
