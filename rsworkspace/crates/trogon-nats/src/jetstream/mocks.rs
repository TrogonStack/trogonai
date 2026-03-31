use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_nats::HeaderMap;
use async_nats::jetstream::AckKind;
use async_nats::jetstream::consumer::pull;
use async_nats::jetstream::publish::PublishAck;
use async_nats::jetstream::stream;
use async_nats::subject::ToSubject;
use bytes::Bytes;
use futures::channel::mpsc;
use futures::stream::BoxStream;

use super::message::{JsAck, JsAckWith, JsDoubleAck, JsDoubleAckWith, JsMessageRef};
use super::traits::{
    JetStreamConsumer, JetStreamConsumerFactory, JetStreamContext, JetStreamPublisher,
};
use crate::mocks::MockError;

// --- MockJsMessage ---

pub struct MockJsMessage {
    inner: async_nats::Message,
    signals: Arc<Mutex<Vec<AckKindSnapshot>>>,
    fail_signals: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AckKindSnapshot {
    Ack,
    AckWith(AckKindValue),
    DoubleAck,
    DoubleAckWith(AckKindValue),
}

#[derive(Debug, Clone, PartialEq)]
pub enum AckKindValue {
    Ack,
    Nak(Option<std::time::Duration>),
    Progress,
    Next,
    Term,
}

impl From<AckKind> for AckKindValue {
    fn from(kind: AckKind) -> Self {
        match kind {
            AckKind::Ack => AckKindValue::Ack,
            AckKind::Nak(d) => AckKindValue::Nak(d),
            AckKind::Progress => AckKindValue::Progress,
            AckKind::Next => AckKindValue::Next,
            AckKind::Term => AckKindValue::Term,
        }
    }
}

impl MockJsMessage {
    pub fn new(message: async_nats::Message) -> Self {
        Self {
            inner: message,
            signals: Arc::new(Mutex::new(Vec::new())),
            fail_signals: false,
        }
    }

    pub fn with_failing_signals(message: async_nats::Message) -> Self {
        Self {
            inner: message,
            signals: Arc::new(Mutex::new(Vec::new())),
            fail_signals: true,
        }
    }

    pub fn signals(&self) -> Vec<AckKindSnapshot> {
        self.signals.lock().unwrap().clone()
    }

    fn record(&self, signal: AckKindSnapshot) {
        self.signals.lock().unwrap().push(signal);
    }
}

impl JsMessageRef for MockJsMessage {
    fn message(&self) -> &async_nats::Message {
        &self.inner
    }
}

impl JsAck for MockJsMessage {
    type Error = MockError;

    async fn ack(&self) -> Result<(), MockError> {
        self.record(AckKindSnapshot::Ack);
        if self.fail_signals {
            Err(MockError("ack failed".into()))
        } else {
            Ok(())
        }
    }
}

impl JsAckWith for MockJsMessage {
    type Error = MockError;

    async fn ack_with(&self, kind: AckKind) -> Result<(), MockError> {
        self.record(AckKindSnapshot::AckWith(kind.into()));
        if self.fail_signals {
            Err(MockError("ack_with failed".into()))
        } else {
            Ok(())
        }
    }
}

impl JsDoubleAck for MockJsMessage {
    type Error = MockError;

    async fn double_ack(&self) -> Result<(), MockError> {
        self.record(AckKindSnapshot::DoubleAck);
        if self.fail_signals {
            Err(MockError("double_ack failed".into()))
        } else {
            Ok(())
        }
    }
}

impl JsDoubleAckWith for MockJsMessage {
    type Error = MockError;

    async fn double_ack_with(&self, kind: AckKind) -> Result<(), MockError> {
        self.record(AckKindSnapshot::DoubleAckWith(kind.into()));
        if self.fail_signals {
            Err(MockError("double_ack_with failed".into()))
        } else {
            Ok(())
        }
    }
}

// --- MockJetStreamContext ---

#[derive(Clone, Debug)]
pub struct MockJetStreamContext {
    created_streams: Arc<Mutex<Vec<stream::Config>>>,
    should_fail: Arc<Mutex<bool>>,
}

impl MockJetStreamContext {
    pub fn new() -> Self {
        Self {
            created_streams: Arc::new(Mutex::new(Vec::new())),
            should_fail: Arc::new(Mutex::new(false)),
        }
    }

    pub fn created_streams(&self) -> Vec<stream::Config> {
        self.created_streams.lock().unwrap().clone()
    }

    pub fn fail_next(&self) {
        *self.should_fail.lock().unwrap() = true;
    }
}

impl Default for MockJetStreamContext {
    fn default() -> Self {
        Self::new()
    }
}

impl JetStreamContext for MockJetStreamContext {
    type Error = MockError;

    async fn get_or_create_stream<S: Into<stream::Config> + Send>(
        &self,
        config: S,
    ) -> Result<(), MockError> {
        let config = config.into();
        let should_fail = {
            let mut flag = self.should_fail.lock().unwrap();
            if *flag {
                *flag = false;
                true
            } else {
                false
            }
        };
        if should_fail {
            return Err(MockError("simulated stream creation failure".to_string()));
        }
        self.created_streams.lock().unwrap().push(config);
        Ok(())
    }
}

// --- MockJetStreamPublisher ---

#[derive(Clone, Debug)]
pub struct MockPublishedJsMessage {
    pub subject: String,
    pub headers: HeaderMap,
    pub payload: Bytes,
}

#[derive(Clone, Debug)]
pub struct MockJetStreamPublisher {
    published: Arc<Mutex<Vec<MockPublishedJsMessage>>>,
    publish_fail_count: Arc<Mutex<u32>>,
    next_sequence: Arc<Mutex<u64>>,
}

impl MockJetStreamPublisher {
    pub fn new() -> Self {
        Self {
            published: Arc::new(Mutex::new(Vec::new())),
            publish_fail_count: Arc::new(Mutex::new(0)),
            next_sequence: Arc::new(Mutex::new(1)),
        }
    }

    pub fn fail_next_js_publish(&self) {
        *self.publish_fail_count.lock().unwrap() = 1;
    }

    pub fn fail_js_publish_count(&self, n: u32) {
        *self.publish_fail_count.lock().unwrap() = n;
    }

    pub fn published_subjects(&self) -> Vec<String> {
        self.published
            .lock()
            .unwrap()
            .iter()
            .map(|m| m.subject.clone())
            .collect()
    }

    pub fn published_payloads(&self) -> Vec<Bytes> {
        self.published
            .lock()
            .unwrap()
            .iter()
            .map(|m| m.payload.clone())
            .collect()
    }
}

impl Default for MockJetStreamPublisher {
    fn default() -> Self {
        Self::new()
    }
}

impl JetStreamPublisher for MockJetStreamPublisher {
    type PublishError = MockError;

    async fn js_publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<PublishAck, MockError> {
        let subject = subject.to_subject().to_string();
        let should_fail = {
            let mut count = self.publish_fail_count.lock().unwrap();
            if *count > 0 {
                *count -= 1;
                true
            } else {
                false
            }
        };
        if should_fail {
            return Err(MockError("simulated js publish failure".to_string()));
        }

        self.published.lock().unwrap().push(MockPublishedJsMessage {
            subject,
            headers,
            payload,
        });

        let seq = {
            let mut seq = self.next_sequence.lock().unwrap();
            let current = *seq;
            *seq += 1;
            current
        };

        Ok(PublishAck {
            stream: "mock-stream".to_string(),
            sequence: seq,
            domain: String::new(),
            duplicate: false,
            value: None,
        })
    }
}

// --- MockJetStreamConsumer ---

pub struct MockJetStreamConsumerFactory {
    consumers: Arc<Mutex<VecDeque<MockJetStreamConsumer>>>,
}

impl Clone for MockJetStreamConsumerFactory {
    fn clone(&self) -> Self {
        Self {
            consumers: self.consumers.clone(),
        }
    }
}

impl MockJetStreamConsumerFactory {
    pub fn new() -> Self {
        Self {
            consumers: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub fn add_consumer(&self, consumer: MockJetStreamConsumer) {
        self.consumers.lock().unwrap().push_back(consumer);
    }
}

impl Default for MockJetStreamConsumerFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl JetStreamConsumerFactory for MockJetStreamConsumerFactory {
    type Error = MockError;
    type Consumer = MockJetStreamConsumer;

    async fn create_consumer(
        &self,
        _stream_name: &str,
        _config: pull::Config,
    ) -> Result<MockJetStreamConsumer, MockError> {
        self.consumers
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| MockError("no mock consumer available".to_string()))
    }
}

pub struct MockJetStreamConsumer {
    rx: Mutex<Option<mpsc::UnboundedReceiver<Result<MockJsMessage, MockError>>>>,
}

impl MockJetStreamConsumer {
    pub fn new() -> (
        Self,
        mpsc::UnboundedSender<Result<MockJsMessage, MockError>>,
    ) {
        let (tx, rx) = mpsc::unbounded();
        (
            Self {
                rx: Mutex::new(Some(rx)),
            },
            tx,
        )
    }

    pub fn failing() -> Self {
        Self {
            rx: Mutex::new(None),
        }
    }
}

impl JetStreamConsumer for MockJetStreamConsumer {
    type Error = MockError;
    type Message = MockJsMessage;
    type Messages = BoxStream<'static, Result<MockJsMessage, MockError>>;

    async fn messages(&self) -> Result<Self::Messages, MockError> {
        use futures::StreamExt;
        let rx = self
            .rx
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| MockError("messages() already called".to_string()))?;
        Ok(rx.boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    fn make_nats_msg(subject: &str, payload: &[u8]) -> async_nats::Message {
        async_nats::Message {
            subject: subject.into(),
            reply: None,
            payload: Bytes::from(payload.to_vec()),
            headers: None,
            status: None,
            description: None,
            length: payload.len(),
        }
    }

    #[tokio::test]
    async fn mock_js_message_records_signals() {
        let msg = MockJsMessage::new(make_nats_msg("test", b"payload"));
        msg.ack().await.unwrap();
        msg.ack_with(AckKind::Progress).await.unwrap();
        msg.ack_with(AckKind::Term).await.unwrap();
        assert_eq!(
            msg.signals(),
            vec![
                AckKindSnapshot::Ack,
                AckKindSnapshot::AckWith(AckKindValue::Progress),
                AckKindSnapshot::AckWith(AckKindValue::Term),
            ]
        );
    }

    #[tokio::test]
    async fn mock_js_message_records_nak_with_delay() {
        let msg = MockJsMessage::new(make_nats_msg("test", b""));
        msg.ack_with(AckKind::Nak(Some(std::time::Duration::from_secs(5))))
            .await
            .unwrap();
        assert_eq!(
            msg.signals(),
            vec![AckKindSnapshot::AckWith(AckKindValue::Nak(Some(
                std::time::Duration::from_secs(5)
            )))]
        );
    }

    #[tokio::test]
    async fn mock_js_message_records_double_ack() {
        let msg = MockJsMessage::new(make_nats_msg("test", b""));
        msg.double_ack().await.unwrap();
        assert_eq!(msg.signals(), vec![AckKindSnapshot::DoubleAck]);
    }

    #[tokio::test]
    async fn mock_js_message_records_double_ack_with() {
        let msg = MockJsMessage::new(make_nats_msg("test", b""));
        msg.double_ack_with(AckKind::Ack).await.unwrap();
        assert_eq!(
            msg.signals(),
            vec![AckKindSnapshot::DoubleAckWith(AckKindValue::Ack)]
        );
    }

    #[tokio::test]
    async fn mock_context_records_stream_creation() {
        let ctx = MockJetStreamContext::new();
        let config = stream::Config {
            name: "TEST_STREAM".to_string(),
            subjects: vec!["test.>".to_string()],
            ..Default::default()
        };
        ctx.get_or_create_stream(config).await.unwrap();
        assert_eq!(ctx.created_streams().len(), 1);
        assert_eq!(ctx.created_streams()[0].name, "TEST_STREAM");
    }

    #[tokio::test]
    async fn mock_context_fails_when_configured() {
        let ctx = MockJetStreamContext::new();
        ctx.fail_next();
        let result = ctx.get_or_create_stream(stream::Config::default()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn mock_publisher_records_publishes() {
        let pub_mock = MockJetStreamPublisher::new();
        let ack = pub_mock
            .js_publish_with_headers(
                "test.subject".to_string(),
                HeaderMap::new(),
                Bytes::from("hello"),
            )
            .await
            .unwrap();
        assert_eq!(ack.sequence, 1);
        assert_eq!(pub_mock.published_subjects(), vec!["test.subject"]);
        assert_eq!(pub_mock.published_payloads(), vec![Bytes::from("hello")]);
    }

    #[tokio::test]
    async fn mock_publisher_increments_sequence() {
        let pub_mock = MockJetStreamPublisher::new();
        let ack1 = pub_mock
            .js_publish_with_headers("a".to_string(), HeaderMap::new(), Bytes::new())
            .await
            .unwrap();
        let ack2 = pub_mock
            .js_publish_with_headers("b".to_string(), HeaderMap::new(), Bytes::new())
            .await
            .unwrap();
        assert_eq!(ack1.sequence, 1);
        assert_eq!(ack2.sequence, 2);
    }

    #[tokio::test]
    async fn mock_publisher_fails_when_configured() {
        let pub_mock = MockJetStreamPublisher::new();
        pub_mock.fail_next_js_publish();
        let result = pub_mock
            .js_publish_with_headers("test".to_string(), HeaderMap::new(), Bytes::new())
            .await;
        assert!(result.is_err());

        let result = pub_mock
            .js_publish_with_headers("test".to_string(), HeaderMap::new(), Bytes::new())
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn mock_consumer_factory_returns_consumers_in_order() {
        let factory = MockJetStreamConsumerFactory::new();
        let (consumer1, _tx1) = MockJetStreamConsumer::new();
        let (consumer2, _tx2) = MockJetStreamConsumer::new();
        factory.add_consumer(consumer1);
        factory.add_consumer(consumer2);

        let _c1 =
            JetStreamConsumerFactory::create_consumer(&factory, "stream", pull::Config::default())
                .await
                .unwrap();
        let _c2 =
            JetStreamConsumerFactory::create_consumer(&factory, "stream", pull::Config::default())
                .await
                .unwrap();

        let result =
            JetStreamConsumerFactory::create_consumer(&factory, "stream", pull::Config::default())
                .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn mock_consumer_messages_streams() {
        let (consumer, tx) = MockJetStreamConsumer::new();
        let msg = MockJsMessage::new(make_nats_msg("test.subject", b"data"));
        tx.unbounded_send(Ok(msg)).unwrap();
        drop(tx);

        let mut stream = JetStreamConsumer::messages(&consumer).await.unwrap();
        let received = stream.next().await.unwrap().unwrap();
        assert_eq!(received.message().subject.as_str(), "test.subject");
        assert_eq!(received.message().payload.as_ref(), b"data");
    }

    #[test]
    fn mock_context_default() {
        let ctx = MockJetStreamContext::default();
        assert!(ctx.created_streams().is_empty());
    }

    #[test]
    fn mock_publisher_default() {
        let pub_mock = MockJetStreamPublisher::default();
        assert!(pub_mock.published_subjects().is_empty());
    }

    #[test]
    fn mock_consumer_factory_default() {
        let _factory = MockJetStreamConsumerFactory::default();
    }

    #[tokio::test]
    async fn mock_js_message_records_nak() {
        let msg = MockJsMessage::new(make_nats_msg("test", b""));
        msg.ack_with(AckKind::Nak(None)).await.unwrap();
        assert_eq!(
            msg.signals(),
            vec![AckKindSnapshot::AckWith(AckKindValue::Nak(None))]
        );
    }

    #[tokio::test]
    async fn mock_publisher_fail_js_publish_count() {
        let pub_mock = MockJetStreamPublisher::new();
        pub_mock.fail_js_publish_count(2);
        assert!(
            pub_mock
                .js_publish_with_headers("a".to_string(), HeaderMap::new(), Bytes::new())
                .await
                .is_err()
        );
        assert!(
            pub_mock
                .js_publish_with_headers("b".to_string(), HeaderMap::new(), Bytes::new())
                .await
                .is_err()
        );
        assert!(
            pub_mock
                .js_publish_with_headers("c".to_string(), HeaderMap::new(), Bytes::new())
                .await
                .is_ok()
        );
    }

    #[test]
    fn mock_js_message_payload_subject_headers_reply() {
        let mut headers = async_nats::HeaderMap::new();
        headers.insert("X-Test", "value");
        let msg = async_nats::Message {
            subject: "test.subject".into(),
            reply: Some("_INBOX.reply".into()),
            payload: Bytes::from("hello"),
            headers: Some(headers),
            status: None,
            description: None,
            length: 5,
        };
        let mock = MockJsMessage::new(msg);

        let inner = mock.message();
        assert_eq!(inner.payload.as_ref(), b"hello");
        assert_eq!(inner.subject.as_str(), "test.subject");
        assert!(inner.headers.is_some());
        assert_eq!(
            inner.reply.as_ref().map(|s| s.as_str()),
            Some("_INBOX.reply")
        );
    }

    #[test]
    fn mock_js_message_no_headers_no_reply() {
        let msg = make_nats_msg("sub", b"data");
        let mock = MockJsMessage::new(msg);

        let inner = mock.message();
        assert!(inner.headers.is_none());
        assert!(inner.reply.is_none());
    }

    #[test]
    fn mock_consumer_factory_clone() {
        let factory = MockJetStreamConsumerFactory::new();
        let (consumer, _tx) = MockJetStreamConsumer::new();
        factory.add_consumer(consumer);
        let cloned = factory.clone();
        assert!(cloned.consumers.lock().unwrap().len() == 1);
    }

    #[tokio::test]
    async fn mock_js_message_failing_signals() {
        let msg = MockJsMessage::with_failing_signals(make_nats_msg("test", b""));
        assert!(msg.ack().await.is_err());
        assert!(msg.ack_with(AckKind::Term).await.is_err());
        assert!(msg.double_ack().await.is_err());
        assert!(msg.double_ack_with(AckKind::Ack).await.is_err());
    }

    #[tokio::test]
    async fn mock_consumer_messages_returns_stream() {
        let (consumer, _tx) = MockJetStreamConsumer::new();
        let result = JetStreamConsumer::messages(&consumer).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn mock_consumer_messages_called_twice_returns_error() {
        let (consumer, _tx) = MockJetStreamConsumer::new();
        let _first = JetStreamConsumer::messages(&consumer).await.unwrap();
        let second = JetStreamConsumer::messages(&consumer).await;
        assert!(second.is_err());
    }

    #[tokio::test]
    async fn mock_js_message_ack_with_next() {
        let msg = MockJsMessage::new(make_nats_msg("test", b""));
        msg.ack_with(AckKind::Next).await.unwrap();
        assert_eq!(
            msg.signals(),
            vec![AckKindSnapshot::AckWith(AckKindValue::Next)]
        );
    }
}
