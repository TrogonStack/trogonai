use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_nats::HeaderMap;
use async_nats::jetstream::consumer::pull;
use async_nats::jetstream::context::{self, CreateKeyValueError, CreateKeyValueErrorKind};
use async_nats::jetstream::kv;
use async_nats::jetstream::publish::PublishAck;
use async_nats::jetstream::stream;
use async_nats::jetstream::{self, AckKind};
use async_nats::subject::ToSubject;
use bytes::Bytes;
use futures::channel::mpsc;
use futures::stream::BoxStream;

use super::message::{JsAck, JsAckWith, JsDoubleAck, JsDoubleAckWith, JsMessageRef};
use super::object_store::{ObjectStoreGet, ObjectStorePut};
use super::traits::{
    JetStreamConsumer, JetStreamContext, JetStreamCreateConsumer, JetStreamCreateKeyValue, JetStreamGetKeyValue,
    JetStreamGetStream, JetStreamKeyValueCreateWithTtl, JetStreamKeyValueDeleteExpectRevision, JetStreamKeyValueStatus,
    JetStreamKeyValueUpdate, JetStreamPublisher,
};
use crate::mocks::MockError;

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
    type Stream = ();

    async fn get_or_create_stream<S: Into<stream::Config> + Send>(&self, config: S) -> Result<(), MockError> {
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

    pub fn published_messages(&self) -> Vec<MockPublishedJsMessage> {
        self.published.lock().unwrap().clone()
    }
}

impl Default for MockJetStreamPublisher {
    fn default() -> Self {
        Self::new()
    }
}

impl JetStreamPublisher for MockJetStreamPublisher {
    type PublishError = MockError;
    type AckFuture = std::future::Ready<Result<PublishAck, MockError>>;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<Self::AckFuture, MockError> {
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

        Ok(std::future::ready(Ok(PublishAck {
            stream: "mock-stream".to_string(),
            sequence: seq,
            domain: String::new(),
            duplicate: false,
            value: None,
        })))
    }
}

#[derive(Clone, Debug)]
pub struct MockJetStreamKvStore {
    settings: Arc<Mutex<Result<MockKeyValueSettings, kv::StatusErrorKind>>>,
    create_with_ttl_result: Arc<Mutex<Result<u64, kv::CreateErrorKind>>>,
    update_result: Arc<Mutex<Result<u64, kv::UpdateErrorKind>>>,
    delete_result: Arc<Mutex<Result<(), kv::DeleteErrorKind>>>,
    create_with_ttl_calls: Arc<Mutex<CreateWithTtlCalls>>,
    update_calls: Arc<Mutex<UpdateCalls>>,
    delete_calls: Arc<Mutex<DeleteCalls>>,
}

#[derive(Clone, Copy, Debug)]
struct MockKeyValueSettings {
    history: i64,
    max_age: Duration,
    allow_message_ttl: bool,
    subject_delete_marker_ttl: Option<Duration>,
}

type CreateWithTtlCalls = Vec<(String, Bytes, Duration)>;
type UpdateCalls = Vec<(String, Bytes, u64)>;
type DeleteCalls = Vec<(String, Option<u64>)>;

impl MockJetStreamKvStore {
    pub fn new() -> Self {
        Self {
            settings: Arc::new(Mutex::new(Ok(MockKeyValueSettings {
                history: 1,
                max_age: Duration::ZERO,
                allow_message_ttl: true,
                subject_delete_marker_ttl: None,
            }))),
            create_with_ttl_result: Arc::new(Mutex::new(Ok(1))),
            update_result: Arc::new(Mutex::new(Ok(1))),
            delete_result: Arc::new(Mutex::new(Ok(()))),
            create_with_ttl_calls: Arc::new(Mutex::new(Vec::new())),
            update_calls: Arc::new(Mutex::new(Vec::new())),
            delete_calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn set_settings(
        &self,
        history: i64,
        max_age: Duration,
        allow_message_ttl: bool,
        subject_delete_marker_ttl: Option<Duration>,
    ) {
        *self.settings.lock().unwrap() = Ok(MockKeyValueSettings {
            history,
            max_age,
            allow_message_ttl,
            subject_delete_marker_ttl,
        });
    }

    pub fn fail_status(&self, kind: kv::StatusErrorKind) {
        *self.settings.lock().unwrap() = Err(kind);
    }

    pub fn set_create_with_ttl_result(&self, result: Result<u64, kv::CreateErrorKind>) {
        *self.create_with_ttl_result.lock().unwrap() = result;
    }

    pub fn set_update_result(&self, result: Result<u64, kv::UpdateErrorKind>) {
        *self.update_result.lock().unwrap() = result;
    }

    pub fn set_delete_result(&self, result: Result<(), kv::DeleteErrorKind>) {
        *self.delete_result.lock().unwrap() = result;
    }

    pub fn create_with_ttl_calls(&self) -> Vec<(String, Bytes, Duration)> {
        self.create_with_ttl_calls.lock().unwrap().clone()
    }

    pub fn update_calls(&self) -> Vec<(String, Bytes, u64)> {
        self.update_calls.lock().unwrap().clone()
    }

    pub fn delete_calls(&self) -> Vec<(String, Option<u64>)> {
        self.delete_calls.lock().unwrap().clone()
    }
}

impl Default for MockJetStreamKvStore {
    fn default() -> Self {
        Self::new()
    }
}

impl JetStreamKeyValueStatus for MockJetStreamKvStore {
    async fn status(&self) -> Result<async_nats::jetstream::kv::bucket::Status, kv::StatusError> {
        self.settings
            .lock()
            .unwrap()
            .clone()
            .map(|settings| status_from_settings("bucket", settings))
            .map_err(kv::StatusError::new)
    }
}

impl JetStreamKeyValueCreateWithTtl for MockJetStreamKvStore {
    async fn create_with_ttl(&self, key: &str, value: Bytes, ttl: Duration) -> Result<u64, kv::CreateError> {
        self.create_with_ttl_calls
            .lock()
            .unwrap()
            .push((key.to_string(), value, ttl));
        (*self.create_with_ttl_result.lock().unwrap()).map_err(kv::CreateError::new)
    }
}

impl JetStreamKeyValueUpdate for MockJetStreamKvStore {
    async fn update(&self, key: &str, value: Bytes, revision: u64) -> Result<u64, kv::UpdateError> {
        self.update_calls
            .lock()
            .unwrap()
            .push((key.to_string(), value, revision));
        (*self.update_result.lock().unwrap()).map_err(kv::UpdateError::new)
    }
}

impl JetStreamKeyValueDeleteExpectRevision for MockJetStreamKvStore {
    async fn delete_expect_revision(&self, key: &str, revision: Option<u64>) -> Result<(), kv::DeleteError> {
        self.delete_calls.lock().unwrap().push((key.to_string(), revision));
        (*self.delete_result.lock().unwrap()).map_err(kv::DeleteError::new)
    }
}

#[derive(Clone, Debug)]
pub struct MockJetStreamKvClient {
    create_result: Arc<Mutex<MockCreateKeyValueResult>>,
    get_result: Arc<Mutex<MockGetKeyValueResult>>,
    create_configs: Arc<Mutex<Vec<kv::Config>>>,
    requested_buckets: Arc<Mutex<Vec<String>>>,
}

#[derive(Clone, Debug)]
enum MockCreateKeyValueResult {
    Ok(MockJetStreamKvStore),
    Err(CreateKeyValueErrorKind),
    AlreadyExists,
}

#[derive(Clone, Debug)]
enum MockGetKeyValueResult {
    Ok(MockJetStreamKvStore),
    Err(context::KeyValueErrorKind),
}

impl MockJetStreamKvClient {
    pub fn new() -> Self {
        Self {
            create_result: Arc::new(Mutex::new(MockCreateKeyValueResult::Ok(MockJetStreamKvStore::new()))),
            get_result: Arc::new(Mutex::new(MockGetKeyValueResult::Ok(MockJetStreamKvStore::new()))),
            create_configs: Arc::new(Mutex::new(Vec::new())),
            requested_buckets: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn set_create_result(&self, result: MockJetStreamKvStore) {
        *self.create_result.lock().unwrap() = MockCreateKeyValueResult::Ok(result);
    }

    pub fn set_get_result(&self, result: MockJetStreamKvStore) {
        *self.get_result.lock().unwrap() = MockGetKeyValueResult::Ok(result);
    }

    pub fn fail_create_already_exists(&self) {
        *self.create_result.lock().unwrap() = MockCreateKeyValueResult::AlreadyExists;
    }

    pub fn fail_create(&self, kind: CreateKeyValueErrorKind) {
        *self.create_result.lock().unwrap() = MockCreateKeyValueResult::Err(kind);
    }

    pub fn fail_get(&self, kind: context::KeyValueErrorKind) {
        *self.get_result.lock().unwrap() = MockGetKeyValueResult::Err(kind);
    }

    pub fn create_configs(&self) -> Vec<kv::Config> {
        self.create_configs.lock().unwrap().clone()
    }

    pub fn requested_buckets(&self) -> Vec<String> {
        self.requested_buckets.lock().unwrap().clone()
    }
}

impl Default for MockJetStreamKvClient {
    fn default() -> Self {
        Self::new()
    }
}

impl JetStreamCreateKeyValue for MockJetStreamKvClient {
    type Store = MockJetStreamKvStore;

    async fn create_key_value(&self, config: kv::Config) -> Result<Self::Store, CreateKeyValueError> {
        self.create_configs.lock().unwrap().push(config);
        match self.create_result.lock().unwrap().clone() {
            MockCreateKeyValueResult::Ok(store) => Ok(store),
            MockCreateKeyValueResult::Err(kind) => Err(CreateKeyValueError::new(kind)),
            MockCreateKeyValueResult::AlreadyExists => Err(CreateKeyValueError::with_source(
                CreateKeyValueErrorKind::BucketCreate,
                mock_stream_exists_error(),
            )),
        }
    }
}

impl JetStreamGetKeyValue for MockJetStreamKvClient {
    type Store = MockJetStreamKvStore;

    async fn get_key_value<T: Into<String> + Send>(&self, bucket: T) -> Result<Self::Store, context::KeyValueError> {
        self.requested_buckets.lock().unwrap().push(bucket.into());
        match self.get_result.lock().unwrap().clone() {
            MockGetKeyValueResult::Ok(store) => Ok(store),
            MockGetKeyValueResult::Err(kind) => Err(context::KeyValueError::new(kind)),
        }
    }
}

fn mock_stream_exists_error() -> context::CreateStreamError {
    let source: jetstream::Error = serde_json::from_str(
        r#"{"code":400,"err_code":10058,"description":"stream name already in use with a different configuration"}"#,
    )
    .unwrap();

    context::CreateStreamError::new(context::CreateStreamErrorKind::JetStream(source))
}

fn status_from_settings(bucket: &str, settings: MockKeyValueSettings) -> async_nats::jetstream::kv::bucket::Status {
    let config = stream::Config {
        name: format!("KV_{bucket}"),
        max_messages_per_subject: settings.history,
        max_age: settings.max_age,
        allow_message_ttl: settings.allow_message_ttl,
        subject_delete_marker_ttl: settings.subject_delete_marker_ttl,
        ..Default::default()
    };

    let info: stream::Info = serde_json::from_value(serde_json::json!({
        "config": config,
        "created": "1970-01-01T00:00:00Z",
        "state": {
            "messages": 0_u64,
            "bytes": 0_u64,
            "first_seq": 0_u64,
            "first_ts": "1970-01-01T00:00:00Z",
            "last_seq": 0_u64,
            "last_ts": "1970-01-01T00:00:00Z",
            "consumer_count": 0_usize,
            "num_subjects": 0_u64
        },
        "cluster": null,
        "mirror": null,
        "sources": []
    }))
    .unwrap();

    async_nats::jetstream::kv::bucket::Status {
        info,
        bucket: bucket.to_string(),
    }
}

pub struct MockJetStreamConsumerFactory {
    consumers: Arc<Mutex<VecDeque<MockJetStreamConsumer>>>,
    get_stream_fail_at: Arc<Mutex<Option<u32>>>,
    get_stream_call_count: Arc<Mutex<u32>>,
}

impl Clone for MockJetStreamConsumerFactory {
    fn clone(&self) -> Self {
        Self {
            consumers: self.consumers.clone(),
            get_stream_fail_at: self.get_stream_fail_at.clone(),
            get_stream_call_count: self.get_stream_call_count.clone(),
        }
    }
}

impl MockJetStreamConsumerFactory {
    pub fn new() -> Self {
        Self {
            consumers: Arc::new(Mutex::new(VecDeque::new())),
            get_stream_fail_at: Arc::new(Mutex::new(None)),
            get_stream_call_count: Arc::new(Mutex::new(0)),
        }
    }

    pub fn add_consumer(&self, consumer: MockJetStreamConsumer) {
        self.consumers.lock().unwrap().push_back(consumer);
    }

    pub fn fail_get_stream_at(&self, call_number: u32) {
        *self.get_stream_fail_at.lock().unwrap() = Some(call_number);
    }
}

impl Default for MockJetStreamConsumerFactory {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MockJetStreamStream {
    consumers: Arc<Mutex<VecDeque<MockJetStreamConsumer>>>,
}

impl JetStreamGetStream for MockJetStreamConsumerFactory {
    type Error = MockError;
    type Stream = MockJetStreamStream;

    async fn get_stream<T: AsRef<str> + Send>(&self, _stream_name: T) -> Result<MockJetStreamStream, MockError> {
        let mut count = self.get_stream_call_count.lock().unwrap();
        *count += 1;
        let current = *count;
        drop(count);

        if let Some(fail_at) = *self.get_stream_fail_at.lock().unwrap()
            && current == fail_at
        {
            return Err(MockError("simulated get_stream failure".to_string()));
        }
        Ok(MockJetStreamStream {
            consumers: self.consumers.clone(),
        })
    }
}

impl JetStreamCreateConsumer for MockJetStreamStream {
    type Error = MockError;
    type Consumer = MockJetStreamConsumer;

    async fn create_consumer(&self, _config: pull::Config) -> Result<MockJetStreamConsumer, MockError> {
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
    pub fn new() -> (Self, mpsc::UnboundedSender<Result<MockJsMessage, MockError>>) {
        let (tx, rx) = mpsc::unbounded();
        (
            Self {
                rx: Mutex::new(Some(rx)),
            },
            tx,
        )
    }

    pub fn failing() -> Self {
        Self { rx: Mutex::new(None) }
    }
}

impl JetStreamConsumer for MockJetStreamConsumer {
    type MessagesError = MockError;
    type StreamError = MockError;
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

#[derive(Clone, Debug)]
pub struct MockObjectStore {
    objects: Arc<Mutex<Vec<(String, Bytes)>>>,
    put_fail_count: Arc<Mutex<u32>>,
    get_fail_count: Arc<Mutex<u32>>,
}

impl MockObjectStore {
    pub fn new() -> Self {
        Self {
            objects: Arc::new(Mutex::new(Vec::new())),
            put_fail_count: Arc::new(Mutex::new(0)),
            get_fail_count: Arc::new(Mutex::new(0)),
        }
    }

    pub fn fail_next_put(&self) {
        *self.put_fail_count.lock().unwrap() = 1;
    }

    pub fn fail_next_get(&self) {
        *self.get_fail_count.lock().unwrap() = 1;
    }

    pub fn seed(&self, name: &str, data: Bytes) {
        self.objects.lock().unwrap().push((name.to_string(), data));
    }

    pub fn stored_objects(&self) -> Vec<(String, Bytes)> {
        self.objects.lock().unwrap().clone()
    }
}

impl Default for MockObjectStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ObjectStorePut for MockObjectStore {
    type Error = MockError;
    type Info = ();

    async fn put<R: tokio::io::AsyncRead + Unpin + Send>(&self, name: &str, data: &mut R) -> Result<(), MockError> {
        use tokio::io::AsyncReadExt;

        let should_fail = {
            let mut count = self.put_fail_count.lock().unwrap();
            if *count > 0 {
                *count -= 1;
                true
            } else {
                false
            }
        };
        if should_fail {
            return Err(MockError("simulated object store put failure".to_string()));
        }
        let mut buf = Vec::new();
        data.read_to_end(&mut buf).await.map_err(|e| MockError(e.to_string()))?;
        self.objects.lock().unwrap().push((name.to_string(), Bytes::from(buf)));
        Ok(())
    }
}

impl ObjectStoreGet for MockObjectStore {
    type Error = MockError;
    type Reader = std::io::Cursor<Vec<u8>>;

    async fn get(&self, name: &str) -> Result<Self::Reader, MockError> {
        let should_fail = {
            let mut count = self.get_fail_count.lock().unwrap();
            if *count > 0 {
                *count -= 1;
                true
            } else {
                false
            }
        };
        if should_fail {
            return Err(MockError("simulated object store get failure".to_string()));
        }
        self.objects
            .lock()
            .unwrap()
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| std::io::Cursor::new(v.to_vec()))
            .ok_or_else(|| MockError(format!("object not found: {name}")))
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
        assert_eq!(msg.signals(), vec![AckKindSnapshot::DoubleAckWith(AckKindValue::Ack)]);
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
            .publish_with_headers("test.subject".to_string(), HeaderMap::new(), Bytes::from("hello"))
            .await
            .unwrap()
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
            .publish_with_headers("a".to_string(), HeaderMap::new(), Bytes::new())
            .await
            .unwrap()
            .await
            .unwrap();
        let ack2 = pub_mock
            .publish_with_headers("b".to_string(), HeaderMap::new(), Bytes::new())
            .await
            .unwrap()
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
            .publish_with_headers("test".to_string(), HeaderMap::new(), Bytes::new())
            .await;
        assert!(result.is_err());

        let result = pub_mock
            .publish_with_headers("test".to_string(), HeaderMap::new(), Bytes::new())
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

        let stream = JetStreamGetStream::get_stream(&factory, "stream").await.unwrap();
        let _c1 = stream.create_consumer(pull::Config::default()).await.unwrap();
        let _c2 = stream.create_consumer(pull::Config::default()).await.unwrap();

        let result = stream.create_consumer(pull::Config::default()).await;
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
        assert_eq!(msg.signals(), vec![AckKindSnapshot::AckWith(AckKindValue::Nak(None))]);
    }

    #[tokio::test]
    async fn mock_publisher_fail_js_publish_count() {
        let pub_mock = MockJetStreamPublisher::new();
        pub_mock.fail_js_publish_count(2);
        assert!(
            pub_mock
                .publish_with_headers("a".to_string(), HeaderMap::new(), Bytes::new())
                .await
                .is_err()
        );
        assert!(
            pub_mock
                .publish_with_headers("b".to_string(), HeaderMap::new(), Bytes::new())
                .await
                .is_err()
        );
        assert!(
            pub_mock
                .publish_with_headers("c".to_string(), HeaderMap::new(), Bytes::new())
                .await
                .unwrap()
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
        assert_eq!(inner.reply.as_ref().map(|s| s.as_str()), Some("_INBOX.reply"));
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
        assert_eq!(msg.signals(), vec![AckKindSnapshot::AckWith(AckKindValue::Next)]);
    }

    #[test]
    fn mock_object_store_default() {
        let store = MockObjectStore::default();
        assert!(store.stored_objects().is_empty());
    }

    #[tokio::test]
    async fn mock_object_store_fail_next_get() {
        let store = MockObjectStore::new();
        store.seed("key", Bytes::from("data"));
        store.fail_next_get();
        let result = ObjectStoreGet::get(&store, "key").await;
        assert!(result.is_err());

        let result = ObjectStoreGet::get(&store, "key").await;
        assert!(result.is_ok());
    }
}
