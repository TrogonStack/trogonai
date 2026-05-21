use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_nats::jetstream::consumer::pull;
use async_nats::jetstream::context::{
    self, CreateKeyValueError, CreateKeyValueErrorKind, GetStreamError, GetStreamErrorKind, PublishError,
    PublishErrorKind, RequestErrorKind,
};
use async_nats::jetstream::kv;
use async_nats::jetstream::message::{OutboundMessage, StreamMessage};
use async_nats::jetstream::publish::PublishAck;
use async_nats::jetstream::stream::{self, LastRawMessageError, LastRawMessageErrorKind, RawMessageError};
use async_nats::jetstream::{self, AckKind};
use async_nats::subject::ToSubject;
use async_nats::{
    HeaderMap,
    header::{NATS_EXPECTED_LAST_SUBJECT_SEQUENCE, NATS_MESSAGE_ID},
};
use bytes::Bytes;
use futures::channel::mpsc;
use futures::stream::{BoxStream, Iter};
use std::vec::IntoIter as VecIntoIter;
use time::OffsetDateTime;

use super::message::{JsAck, JsAckWith, JsDoubleAck, JsDoubleAckWith, JsMessageRef};
use super::object_store::{ObjectStoreGet, ObjectStorePut};
use super::traits::{
    JetStreamConsumer, JetStreamContext, JetStreamCreateConsumer, JetStreamCreateKeyValue, JetStreamGetKeyValue,
    JetStreamGetRawMessage, JetStreamGetStream, JetStreamGetStreamInfo, JetStreamKeyValueCreateWithTtl,
    JetStreamKeyValueDeleteExpectRevision, JetStreamKeyValueStatus, JetStreamKeyValueUpdate, JetStreamKvCreate,
    JetStreamKvEntry, JetStreamKvGet, JetStreamKvKeys, JetStreamLastRawMessageBySubject, JetStreamPublishMessage,
    JetStreamPublisher,
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

#[derive(Clone, Debug, Default)]
pub struct MockJetStreamPublishMessage {
    published: Arc<Mutex<Vec<MockPublishedOutboundMessage>>>,
    pending_batches: Arc<Mutex<HashMap<String, Vec<MockPublishedOutboundMessage>>>>,
    next_sequence: Arc<Mutex<u64>>,
    results: Arc<Mutex<VecDeque<MockPublishMessageOutcome>>>,
}

#[derive(Clone, Debug)]
pub struct MockPublishedOutboundMessage {
    pub sequence: u64,
    pub subject: String,
    pub headers: Option<HeaderMap>,
    pub payload: Bytes,
}

impl MockPublishedOutboundMessage {
    fn new(message: OutboundMessage, sequence: u64) -> Self {
        Self {
            sequence,
            subject: message.subject.to_string(),
            headers: message.headers,
            payload: message.payload,
        }
    }

    fn stream_message(&self) -> StreamMessage {
        StreamMessage {
            subject: self.subject.clone().into(),
            sequence: self.sequence,
            headers: self.headers.clone().unwrap_or_default(),
            payload: self.payload.clone(),
            time: OffsetDateTime::UNIX_EPOCH,
        }
    }
}

#[derive(Clone, Debug)]
enum MockPublishMessageOutcome {
    Ack(u64),
    Duplicate(u64),
    PublishError(PublishErrorKind),
    AckError(PublishErrorKind),
}

const NATS_BATCH_COMMIT: &str = "Nats-Batch-Commit";
const NATS_BATCH_ID: &str = "Nats-Batch-Id";

impl MockJetStreamPublishMessage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn published_messages(&self) -> Vec<MockPublishedOutboundMessage> {
        self.published.lock().unwrap().clone()
    }

    pub fn last_subject_sequence(&self, subject: &str) -> u64 {
        last_subject_sequence(&self.published.lock().unwrap(), subject)
    }

    pub fn enqueue_wrong_last_sequence(&self) {
        self.results
            .lock()
            .unwrap()
            .push_back(MockPublishMessageOutcome::AckError(PublishErrorKind::WrongLastSequence));
    }

    pub fn enqueue_publish_error(&self, kind: PublishErrorKind) {
        self.results
            .lock()
            .unwrap()
            .push_back(MockPublishMessageOutcome::PublishError(kind));
    }

    pub fn enqueue_ack_error(&self, kind: PublishErrorKind) {
        self.results
            .lock()
            .unwrap()
            .push_back(MockPublishMessageOutcome::AckError(kind));
    }

    pub fn enqueue_duplicate(&self) {
        let seq = self.next_assigned_sequence();
        self.results
            .lock()
            .unwrap()
            .push_back(MockPublishMessageOutcome::Duplicate(seq));
    }

    pub fn enqueue_ack_with_sequence(&self, sequence: u64) {
        {
            let mut next = self.next_sequence.lock().unwrap();
            if sequence >= *next {
                *next = sequence + 1;
            }
        }
        self.results
            .lock()
            .unwrap()
            .push_back(MockPublishMessageOutcome::Ack(sequence));
    }

    fn next_assigned_sequence(&self) -> u64 {
        let mut seq = self.next_sequence.lock().unwrap();
        let current = if *seq == 0 { 1 } else { *seq };
        *seq = current + 1;
        current
    }

    fn publish_message_semantic(
        &self,
        message: OutboundMessage,
    ) -> Result<std::future::Ready<Result<PublishAck, PublishError>>, PublishError> {
        let subject = message.subject.to_string();
        let headers = message.headers.as_ref();
        let batch_id = header_value(headers, NATS_BATCH_ID).map(str::to_string);
        let is_batch_commit = header_value(headers, NATS_BATCH_COMMIT).is_some();

        if let Some(expected_sequence) = expected_last_subject_sequence(headers)? {
            let current_sequence = self.last_subject_sequence(&subject);
            if current_sequence != expected_sequence {
                if let Some(batch_id) = &batch_id {
                    self.pending_batches.lock().unwrap().remove(batch_id);
                }
                return Ok(std::future::ready(Err(PublishError::new(
                    PublishErrorKind::WrongLastSequence,
                ))));
            }
        }

        if let Some(message_id) = message_id(headers)
            && let Some(sequence) = self.duplicate_sequence(message_id)
        {
            if let Some(batch_id) = &batch_id {
                self.pending_batches.lock().unwrap().remove(batch_id);
            }
            return Ok(std::future::ready(Ok(publish_ack(sequence, true))));
        }

        let sequence = self.next_assigned_sequence();
        let stored = MockPublishedOutboundMessage::new(message, sequence);
        match (batch_id, is_batch_commit) {
            (Some(batch_id), false) => {
                self.pending_batches
                    .lock()
                    .unwrap()
                    .entry(batch_id)
                    .or_default()
                    .push(stored);
                Ok(std::future::ready(Err(empty_atomic_batch_member_ack_error())))
            }
            (Some(batch_id), true) => {
                let mut pending = self
                    .pending_batches
                    .lock()
                    .unwrap()
                    .remove(&batch_id)
                    .unwrap_or_default();
                pending.push(stored);
                self.published.lock().unwrap().extend(pending);
                Ok(std::future::ready(Ok(publish_ack(sequence, false))))
            }
            (None, _) => {
                self.published.lock().unwrap().push(stored);
                Ok(std::future::ready(Ok(publish_ack(sequence, false))))
            }
        }
    }

    fn duplicate_sequence(&self, message_id: &str) -> Option<u64> {
        let published = self.published.lock().unwrap();
        let published_sequence = published
            .iter()
            .find(|message| message_id_from_headers(message.headers.as_ref()).as_deref() == Some(message_id))
            .map(|message| message.sequence);
        if published_sequence.is_some() {
            return published_sequence;
        }
        self.pending_batches
            .lock()
            .unwrap()
            .values()
            .flatten()
            .find(|message| message_id_from_headers(message.headers.as_ref()).as_deref() == Some(message_id))
            .map(|message| message.sequence)
    }
}

fn publish_ack(sequence: u64, duplicate: bool) -> PublishAck {
    PublishAck {
        stream: "mock-stream".to_string(),
        sequence,
        domain: String::new(),
        duplicate,
        value: None,
    }
}

fn header_value(headers: Option<&HeaderMap>, name: impl async_nats::header::IntoHeaderName) -> Option<&str> {
    headers.and_then(|headers| headers.get(name).map(|value| value.as_str()))
}

fn expected_last_subject_sequence(headers: Option<&HeaderMap>) -> Result<Option<u64>, PublishError> {
    header_value(headers, NATS_EXPECTED_LAST_SUBJECT_SEQUENCE)
        .map(str::parse)
        .transpose()
        .map_err(|_| PublishError::new(PublishErrorKind::WrongLastSequence))
}

fn message_id(headers: Option<&HeaderMap>) -> Option<&str> {
    header_value(headers, NATS_MESSAGE_ID)
}

fn message_id_from_headers(headers: Option<&HeaderMap>) -> Option<String> {
    message_id(headers).map(str::to_string)
}

fn last_subject_sequence(messages: &[MockPublishedOutboundMessage], subject: &str) -> u64 {
    messages
        .iter()
        .rev()
        .find(|message| message.subject == subject)
        .map(|message| message.sequence)
        .unwrap_or_default()
}

fn empty_atomic_batch_member_ack_error() -> PublishError {
    let source = serde_json::from_str::<PublishAck>("").unwrap_err();
    PublishError::with_source(PublishErrorKind::Other, source)
}

impl JetStreamPublishMessage for MockJetStreamPublishMessage {
    type PublishError = PublishError;
    type AckFuture = std::future::Ready<Result<PublishAck, PublishError>>;

    async fn publish_message(&self, message: OutboundMessage) -> Result<Self::AckFuture, PublishError> {
        let outcome = self.results.lock().unwrap().pop_front();
        match outcome {
            Some(MockPublishMessageOutcome::PublishError(kind)) => Err(PublishError::new(kind)),
            Some(MockPublishMessageOutcome::Ack(sequence)) => {
                self.published
                    .lock()
                    .unwrap()
                    .push(MockPublishedOutboundMessage::new(message, sequence));
                Ok(std::future::ready(Ok(PublishAck {
                    stream: "mock-stream".to_string(),
                    sequence,
                    domain: String::new(),
                    duplicate: false,
                    value: None,
                })))
            }
            Some(MockPublishMessageOutcome::Duplicate(sequence)) => Ok(std::future::ready(Ok(PublishAck {
                stream: "mock-stream".to_string(),
                sequence,
                domain: String::new(),
                duplicate: true,
                value: None,
            }))),
            Some(MockPublishMessageOutcome::AckError(kind)) => {
                let sequence = self.next_assigned_sequence();
                self.published
                    .lock()
                    .unwrap()
                    .push(MockPublishedOutboundMessage::new(message, sequence));
                Ok(std::future::ready(Err(PublishError::new(kind))))
            }
            None => self.publish_message_semantic(message),
        }
    }
}

impl JetStreamLastRawMessageBySubject for MockJetStreamPublishMessage {
    async fn get_last_raw_message_by_subject(&self, subject: &str) -> Result<StreamMessage, LastRawMessageError> {
        self.published
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|message| message.subject == subject)
            .map(MockPublishedOutboundMessage::stream_message)
            .ok_or_else(|| LastRawMessageError::new(LastRawMessageErrorKind::NoMessageFound))
    }
}

impl JetStreamGetStreamInfo for MockJetStreamPublishMessage {
    async fn get_info(&self) -> Result<stream::Info, stream::InfoError> {
        let published = self.published.lock().unwrap();
        let last_sequence = published
            .iter()
            .map(|message| message.sequence)
            .max()
            .unwrap_or_default();
        Ok(mock_stream_info(published.len() as u64, last_sequence))
    }
}

impl JetStreamGetRawMessage for MockJetStreamPublishMessage {
    async fn get_raw_message(&self, sequence: u64) -> Result<StreamMessage, RawMessageError> {
        self.published
            .lock()
            .unwrap()
            .iter()
            .find(|message| message.sequence == sequence)
            .map(MockPublishedOutboundMessage::stream_message)
            .ok_or_else(|| RawMessageError::new(LastRawMessageErrorKind::NoMessageFound))
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
    update_results_queue: Arc<Mutex<VecDeque<Result<u64, kv::UpdateErrorKind>>>>,
    delete_results_queue: Arc<Mutex<VecDeque<Result<(), kv::DeleteErrorKind>>>>,
    entry_results: Arc<Mutex<VecDeque<MockKvEntryOutcome>>>,
    get_results: Arc<Mutex<VecDeque<MockKvGetOutcome>>>,
    create_results: Arc<Mutex<VecDeque<Result<u64, kv::CreateErrorKind>>>>,
    keys_result: Arc<Mutex<Result<Vec<String>, kv::WatchErrorKind>>>,
    entry_calls: Arc<Mutex<Vec<String>>>,
    get_calls: Arc<Mutex<Vec<String>>>,
    create_calls: Arc<Mutex<Vec<(String, Bytes)>>>,
    keys_calls: Arc<Mutex<u32>>,
    bucket_name: Arc<Mutex<String>>,
}

#[derive(Clone, Debug)]
pub enum MockKvEntryOutcome {
    None,
    Some {
        value: Bytes,
        revision: u64,
        operation: kv::Operation,
    },
    Error(kv::EntryErrorKind),
}

#[derive(Clone, Debug)]
pub enum MockKvGetOutcome {
    None,
    Some(Bytes),
    Error(kv::EntryErrorKind),
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
            update_results_queue: Arc::new(Mutex::new(VecDeque::new())),
            delete_results_queue: Arc::new(Mutex::new(VecDeque::new())),
            entry_results: Arc::new(Mutex::new(VecDeque::new())),
            get_results: Arc::new(Mutex::new(VecDeque::new())),
            create_results: Arc::new(Mutex::new(VecDeque::new())),
            keys_result: Arc::new(Mutex::new(Ok(Vec::new()))),
            entry_calls: Arc::new(Mutex::new(Vec::new())),
            get_calls: Arc::new(Mutex::new(Vec::new())),
            create_calls: Arc::new(Mutex::new(Vec::new())),
            keys_calls: Arc::new(Mutex::new(0)),
            bucket_name: Arc::new(Mutex::new("mock-bucket".to_string())),
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

    pub fn set_bucket_name(&self, name: impl Into<String>) {
        *self.bucket_name.lock().unwrap() = name.into();
    }

    pub fn enqueue_update_result(&self, result: Result<u64, kv::UpdateErrorKind>) {
        self.update_results_queue.lock().unwrap().push_back(result);
    }

    pub fn enqueue_delete_result(&self, result: Result<(), kv::DeleteErrorKind>) {
        self.delete_results_queue.lock().unwrap().push_back(result);
    }

    pub fn enqueue_entry_none(&self) {
        self.entry_results.lock().unwrap().push_back(MockKvEntryOutcome::None);
    }

    pub fn enqueue_entry(&self, value: Bytes, revision: u64, operation: kv::Operation) {
        self.entry_results.lock().unwrap().push_back(MockKvEntryOutcome::Some {
            value,
            revision,
            operation,
        });
    }

    pub fn enqueue_entry_error(&self, kind: kv::EntryErrorKind) {
        self.entry_results
            .lock()
            .unwrap()
            .push_back(MockKvEntryOutcome::Error(kind));
    }

    pub fn enqueue_get_none(&self) {
        self.get_results.lock().unwrap().push_back(MockKvGetOutcome::None);
    }

    pub fn enqueue_get_some(&self, value: Bytes) {
        self.get_results
            .lock()
            .unwrap()
            .push_back(MockKvGetOutcome::Some(value));
    }

    pub fn enqueue_get_error(&self, kind: kv::EntryErrorKind) {
        self.get_results
            .lock()
            .unwrap()
            .push_back(MockKvGetOutcome::Error(kind));
    }

    pub fn enqueue_create_result(&self, result: Result<u64, kv::CreateErrorKind>) {
        self.create_results.lock().unwrap().push_back(result);
    }

    pub fn set_keys_result(&self, result: Result<Vec<String>, kv::WatchErrorKind>) {
        *self.keys_result.lock().unwrap() = result;
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

    pub fn entry_calls(&self) -> Vec<String> {
        self.entry_calls.lock().unwrap().clone()
    }

    pub fn get_calls(&self) -> Vec<String> {
        self.get_calls.lock().unwrap().clone()
    }

    pub fn create_calls(&self) -> Vec<(String, Bytes)> {
        self.create_calls.lock().unwrap().clone()
    }

    pub fn keys_calls(&self) -> u32 {
        *self.keys_calls.lock().unwrap()
    }
}

impl Default for MockJetStreamKvStore {
    fn default() -> Self {
        Self::new()
    }
}

impl JetStreamKeyValueStatus for MockJetStreamKvStore {
    async fn status(&self) -> Result<async_nats::jetstream::kv::bucket::Status, kv::StatusError> {
        let bucket = self.bucket_name.lock().unwrap().clone();
        self.settings
            .lock()
            .unwrap()
            .clone()
            .map(|settings| status_from_settings(&bucket, settings))
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
        let queued = self.update_results_queue.lock().unwrap().pop_front();
        match queued {
            Some(result) => result.map_err(kv::UpdateError::new),
            None => (*self.update_result.lock().unwrap()).map_err(kv::UpdateError::new),
        }
    }
}

impl JetStreamKeyValueDeleteExpectRevision for MockJetStreamKvStore {
    async fn delete_expect_revision(&self, key: &str, revision: Option<u64>) -> Result<(), kv::DeleteError> {
        self.delete_calls.lock().unwrap().push((key.to_string(), revision));
        let queued = self.delete_results_queue.lock().unwrap().pop_front();
        match queued {
            Some(result) => result.map_err(kv::DeleteError::new),
            None => (*self.delete_result.lock().unwrap()).map_err(kv::DeleteError::new),
        }
    }
}

impl JetStreamKvGet for MockJetStreamKvStore {
    async fn get(&self, key: String) -> Result<Option<Bytes>, kv::EntryError> {
        self.get_calls.lock().unwrap().push(key);
        let outcome = self.get_results.lock().unwrap().pop_front();
        match outcome {
            Some(MockKvGetOutcome::None) | None => Ok(None),
            Some(MockKvGetOutcome::Some(value)) => Ok(Some(value)),
            Some(MockKvGetOutcome::Error(kind)) => Err(kv::EntryError::new(kind)),
        }
    }
}

impl JetStreamKvEntry for MockJetStreamKvStore {
    async fn entry(&self, key: String) -> Result<Option<kv::Entry>, kv::EntryError> {
        let key_for_entry = key.clone();
        self.entry_calls.lock().unwrap().push(key);
        let outcome = self.entry_results.lock().unwrap().pop_front();
        let bucket = self.bucket_name.lock().unwrap().clone();
        match outcome {
            Some(MockKvEntryOutcome::None) | None => Ok(None),
            Some(MockKvEntryOutcome::Some {
                value,
                revision,
                operation,
            }) => Ok(Some(kv::Entry {
                bucket,
                key: key_for_entry,
                value,
                revision,
                delta: 0,
                created: OffsetDateTime::UNIX_EPOCH,
                operation,
                seen_current: true,
            })),
            Some(MockKvEntryOutcome::Error(kind)) => Err(kv::EntryError::new(kind)),
        }
    }
}

impl JetStreamKvCreate for MockJetStreamKvStore {
    async fn create(&self, key: &str, value: Bytes) -> Result<u64, kv::CreateError> {
        self.create_calls.lock().unwrap().push((key.to_string(), value));
        let queued = self.create_results.lock().unwrap().pop_front();
        match queued {
            Some(result) => result.map_err(kv::CreateError::new),
            None => Ok(1),
        }
    }
}

impl JetStreamKvKeys for MockJetStreamKvStore {
    type Keys = Iter<VecIntoIter<Result<String, kv::WatcherError>>>;

    async fn keys(&self) -> Result<Self::Keys, kv::HistoryError> {
        *self.keys_calls.lock().unwrap() += 1;
        let result = self.keys_result.lock().unwrap().clone();
        match result {
            Ok(keys) => {
                let items: Vec<Result<String, kv::WatcherError>> = keys.into_iter().map(Ok).collect();
                Ok(futures::stream::iter(items.into_iter()))
            }
            Err(kind) => Err(kv::HistoryError::new(kind)),
        }
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

fn mock_stream_info(messages: u64, last_sequence: u64) -> stream::Info {
    let first_sequence = if messages == 0 { 0 } else { 1 };
    serde_json::from_value(serde_json::json!({
        "config": {
            "name": "mock-stream",
            "subjects": [],
            "retention": "limits",
            "max_consumers": -1,
            "max_msgs": -1,
            "max_bytes": -1,
            "discard": "old",
            "max_age": 0,
            "storage": "file",
            "num_replicas": 1
        },
        "created": "1970-01-01T00:00:00Z",
        "state": {
            "messages": messages,
            "bytes": 0_u64,
            "first_seq": first_sequence,
            "first_ts": "1970-01-01T00:00:00Z",
            "last_seq": last_sequence,
            "last_ts": "1970-01-01T00:00:00Z",
            "consumer_count": 0_usize,
            "num_subjects": 0_u64
        },
        "cluster": null,
        "mirror": null,
        "sources": []
    }))
    .unwrap()
}

pub struct MockJetStreamConsumerFactory {
    consumers: Arc<Mutex<VecDeque<MockJetStreamConsumer>>>,
    get_stream_fail_at: Arc<Mutex<Option<u32>>>,
    get_stream_call_count: Arc<Mutex<u32>>,
    get_stream_calls: Arc<Mutex<Vec<String>>>,
    last_raw_messages: Arc<Mutex<VecDeque<Result<StreamMessage, LastRawMessageError>>>>,
    last_raw_message_subjects: Arc<Mutex<Vec<String>>>,
    info_result: Arc<Mutex<Option<stream::Info>>>,
    raw_messages_by_sequence: Arc<Mutex<HashMap<u64, Result<StreamMessage, LastRawMessageErrorKind>>>>,
    raw_message_calls: Arc<Mutex<Vec<u64>>>,
}

impl Clone for MockJetStreamConsumerFactory {
    fn clone(&self) -> Self {
        Self {
            consumers: self.consumers.clone(),
            get_stream_fail_at: self.get_stream_fail_at.clone(),
            get_stream_call_count: self.get_stream_call_count.clone(),
            get_stream_calls: self.get_stream_calls.clone(),
            last_raw_messages: self.last_raw_messages.clone(),
            last_raw_message_subjects: self.last_raw_message_subjects.clone(),
            info_result: self.info_result.clone(),
            raw_messages_by_sequence: self.raw_messages_by_sequence.clone(),
            raw_message_calls: self.raw_message_calls.clone(),
        }
    }
}

impl MockJetStreamConsumerFactory {
    pub fn new() -> Self {
        Self {
            consumers: Arc::new(Mutex::new(VecDeque::new())),
            get_stream_fail_at: Arc::new(Mutex::new(None)),
            get_stream_call_count: Arc::new(Mutex::new(0)),
            get_stream_calls: Arc::new(Mutex::new(Vec::new())),
            last_raw_messages: Arc::new(Mutex::new(VecDeque::new())),
            last_raw_message_subjects: Arc::new(Mutex::new(Vec::new())),
            info_result: Arc::new(Mutex::new(None)),
            raw_messages_by_sequence: Arc::new(Mutex::new(HashMap::new())),
            raw_message_calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn add_consumer(&self, consumer: MockJetStreamConsumer) {
        self.consumers.lock().unwrap().push_back(consumer);
    }

    pub fn fail_get_stream_at(&self, call_number: u32) {
        *self.get_stream_fail_at.lock().unwrap() = Some(call_number);
    }

    pub fn add_last_raw_message(&self, message: StreamMessage) {
        self.last_raw_messages.lock().unwrap().push_back(Ok(message));
    }

    pub fn add_last_raw_message_error(&self, error: LastRawMessageError) {
        self.last_raw_messages.lock().unwrap().push_back(Err(error));
    }

    pub fn set_info(&self, info: stream::Info) {
        *self.info_result.lock().unwrap() = Some(info);
    }

    pub fn add_raw_message(&self, sequence: u64, message: StreamMessage) {
        self.raw_messages_by_sequence
            .lock()
            .unwrap()
            .insert(sequence, Ok(message));
    }

    pub fn add_raw_message_error(&self, sequence: u64, kind: LastRawMessageErrorKind) {
        self.raw_messages_by_sequence
            .lock()
            .unwrap()
            .insert(sequence, Err(kind));
    }

    pub fn get_stream_calls(&self) -> Vec<String> {
        self.get_stream_calls.lock().unwrap().clone()
    }

    pub fn last_raw_message_subjects(&self) -> Vec<String> {
        self.last_raw_message_subjects.lock().unwrap().clone()
    }

    pub fn raw_message_calls(&self) -> Vec<u64> {
        self.raw_message_calls.lock().unwrap().clone()
    }
}

impl Default for MockJetStreamConsumerFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct MockJetStreamStream {
    consumers: Arc<Mutex<VecDeque<MockJetStreamConsumer>>>,
    last_raw_messages: Arc<Mutex<VecDeque<Result<StreamMessage, LastRawMessageError>>>>,
    last_raw_message_subjects: Arc<Mutex<Vec<String>>>,
    info_result: Arc<Mutex<Option<stream::Info>>>,
    raw_messages_by_sequence: Arc<Mutex<HashMap<u64, Result<StreamMessage, LastRawMessageErrorKind>>>>,
    raw_message_calls: Arc<Mutex<Vec<u64>>>,
}

impl MockJetStreamStream {
    pub fn set_info(&self, info: stream::Info) {
        *self.info_result.lock().unwrap() = Some(info);
    }

    pub fn add_raw_message(&self, sequence: u64, message: StreamMessage) {
        self.raw_messages_by_sequence
            .lock()
            .unwrap()
            .insert(sequence, Ok(message));
    }

    pub fn add_raw_message_error(&self, sequence: u64, kind: LastRawMessageErrorKind) {
        self.raw_messages_by_sequence
            .lock()
            .unwrap()
            .insert(sequence, Err(kind));
    }

    pub fn raw_message_calls(&self) -> Vec<u64> {
        self.raw_message_calls.lock().unwrap().clone()
    }
}

impl JetStreamGetStream for MockJetStreamConsumerFactory {
    type Error = GetStreamError;
    type Stream = MockJetStreamStream;

    async fn get_stream<T: AsRef<str> + Send>(&self, stream_name: T) -> Result<MockJetStreamStream, GetStreamError> {
        self.get_stream_calls
            .lock()
            .unwrap()
            .push(stream_name.as_ref().to_string());

        let mut count = self.get_stream_call_count.lock().unwrap();
        *count += 1;
        let current = *count;
        drop(count);

        if let Some(fail_at) = *self.get_stream_fail_at.lock().unwrap()
            && current == fail_at
        {
            return Err(GetStreamError::with_source(
                GetStreamErrorKind::Request,
                MockError("simulated get_stream failure".to_string()),
            ));
        }
        Ok(MockJetStreamStream {
            consumers: self.consumers.clone(),
            last_raw_messages: self.last_raw_messages.clone(),
            last_raw_message_subjects: self.last_raw_message_subjects.clone(),
            info_result: self.info_result.clone(),
            raw_messages_by_sequence: self.raw_messages_by_sequence.clone(),
            raw_message_calls: self.raw_message_calls.clone(),
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

impl JetStreamLastRawMessageBySubject for MockJetStreamStream {
    async fn get_last_raw_message_by_subject(&self, subject: &str) -> Result<StreamMessage, LastRawMessageError> {
        self.last_raw_message_subjects.lock().unwrap().push(subject.to_string());

        self.last_raw_messages
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(LastRawMessageError::new(LastRawMessageErrorKind::NoMessageFound)))
    }
}

impl JetStreamGetStreamInfo for MockJetStreamStream {
    async fn get_info(&self) -> Result<stream::Info, stream::InfoError> {
        match self.info_result.lock().unwrap().clone() {
            Some(info) => Ok(info),
            None => Err(stream::InfoError::new(RequestErrorKind::Other)),
        }
    }
}

impl JetStreamGetRawMessage for MockJetStreamStream {
    async fn get_raw_message(&self, sequence: u64) -> Result<StreamMessage, RawMessageError> {
        self.raw_message_calls.lock().unwrap().push(sequence);
        let entry = self.raw_messages_by_sequence.lock().unwrap().get(&sequence).cloned();
        match entry {
            Some(Ok(message)) => Ok(message),
            Some(Err(kind)) => Err(RawMessageError::new(kind)),
            None => Err(RawMessageError::new(LastRawMessageErrorKind::NoMessageFound)),
        }
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
    async fn mock_publish_message_advances_default_sequence_after_explicit_ack() {
        let publisher = MockJetStreamPublishMessage::new();
        publisher.enqueue_ack_with_sequence(10);

        let first = publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .payload(Bytes::new())
                    .outbound_message("a"),
            )
            .await
            .unwrap()
            .await
            .unwrap();
        let second = publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .payload(Bytes::new())
                    .outbound_message("b"),
            )
            .await
            .unwrap()
            .await
            .unwrap();

        assert_eq!(first.sequence, 10);
        assert_eq!(second.sequence, 11);
    }

    #[tokio::test]
    async fn mock_publish_message_scripts_ack_level_errors() {
        let publisher = MockJetStreamPublishMessage::new();
        publisher.enqueue_wrong_last_sequence();
        publisher.enqueue_ack_error(PublishErrorKind::Other);

        let wrong_last_sequence = publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .payload(Bytes::new())
                    .outbound_message("events.alpha"),
            )
            .await
            .unwrap()
            .await
            .unwrap_err();
        let other = publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .payload(Bytes::new())
                    .outbound_message("events.alpha"),
            )
            .await
            .unwrap()
            .await
            .unwrap_err();

        assert_eq!(wrong_last_sequence.kind(), PublishErrorKind::WrongLastSequence);
        assert_eq!(other.kind(), PublishErrorKind::Other);
        assert_eq!(publisher.published_messages().len(), 2);
    }

    #[tokio::test]
    async fn mock_publish_message_scripts_outer_publish_errors_and_duplicates() {
        let publisher = MockJetStreamPublishMessage::new();
        publisher.enqueue_publish_error(PublishErrorKind::TimedOut);
        publisher.enqueue_duplicate();

        let publish_error = publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .payload(Bytes::new())
                    .outbound_message("events.alpha"),
            )
            .await
            .unwrap_err();
        let duplicate = publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .payload(Bytes::new())
                    .outbound_message("events.alpha"),
            )
            .await
            .unwrap()
            .await
            .unwrap();

        assert_eq!(publish_error.kind(), PublishErrorKind::TimedOut);
        assert_eq!(duplicate.sequence, 1);
        assert!(duplicate.duplicate);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn mock_publish_message_rejects_invalid_expected_sequence_header() {
        let publisher = MockJetStreamPublishMessage::new();

        let error = publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .header(NATS_EXPECTED_LAST_SUBJECT_SEQUENCE, "not-a-number")
                    .payload(Bytes::new())
                    .outbound_message("events.alpha"),
            )
            .await
            .unwrap_err();

        assert_eq!(error.kind(), PublishErrorKind::WrongLastSequence);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn mock_publish_message_enforces_expected_last_subject_sequence() {
        let publisher = MockJetStreamPublishMessage::new();

        publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .message_id("first")
                    .payload(Bytes::new())
                    .outbound_message("events.alpha"),
            )
            .await
            .unwrap()
            .await
            .unwrap();
        publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .message_id("second")
                    .expected_last_subject_sequence(1)
                    .payload(Bytes::new())
                    .outbound_message("events.alpha"),
            )
            .await
            .unwrap()
            .await
            .unwrap();

        let stale = publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .message_id("stale")
                    .expected_last_subject_sequence(1)
                    .payload(Bytes::new())
                    .outbound_message("events.alpha"),
            )
            .await
            .unwrap()
            .await
            .unwrap_err();

        assert_eq!(stale.kind(), PublishErrorKind::WrongLastSequence);
        assert_eq!(publisher.last_subject_sequence("events.alpha"), 2);
    }

    #[tokio::test]
    async fn mock_publish_message_rejects_duplicate_message_id_without_recording() {
        let publisher = MockJetStreamPublishMessage::new();

        let first = publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .message_id("event-1")
                    .payload(Bytes::new())
                    .outbound_message("events.alpha"),
            )
            .await
            .unwrap()
            .await
            .unwrap();
        let duplicate = publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .message_id("event-1")
                    .payload(Bytes::new())
                    .outbound_message("events.alpha"),
            )
            .await
            .unwrap()
            .await
            .unwrap();

        assert_eq!(first.sequence, 1);
        assert_eq!(duplicate.sequence, 1);
        assert!(duplicate.duplicate);
        assert_eq!(publisher.published_messages().len(), 1);
    }

    #[tokio::test]
    async fn mock_publish_message_stages_atomic_batch_until_commit() {
        let publisher = MockJetStreamPublishMessage::new();

        let member = publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .message_id("event-1")
                    .header(NATS_BATCH_ID, "batch-1")
                    .payload(Bytes::from_static(b"one"))
                    .outbound_message("events.alpha"),
            )
            .await
            .unwrap()
            .await
            .unwrap_err();
        assert_eq!(member.kind(), PublishErrorKind::Other);
        assert!(publisher.published_messages().is_empty());

        let commit = publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .message_id("event-2")
                    .header(NATS_BATCH_ID, "batch-1")
                    .header(NATS_BATCH_COMMIT, "1")
                    .payload(Bytes::from_static(b"two"))
                    .outbound_message("events.alpha"),
            )
            .await
            .unwrap()
            .await
            .unwrap();

        assert_eq!(commit.sequence, 2);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].payload, Bytes::from_static(b"one"));
        assert_eq!(messages[1].payload, Bytes::from_static(b"two"));
    }

    #[tokio::test]
    async fn mock_publish_message_drops_pending_batch_on_conflict() {
        let publisher = MockJetStreamPublishMessage::new();

        publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .message_id("event-1")
                    .header(NATS_BATCH_ID, "batch-1")
                    .payload(Bytes::from_static(b"one"))
                    .outbound_message("events.alpha"),
            )
            .await
            .unwrap()
            .await
            .unwrap_err();
        let conflict = publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .message_id("event-2")
                    .expected_last_subject_sequence(99)
                    .header(NATS_BATCH_ID, "batch-1")
                    .payload(Bytes::from_static(b"two"))
                    .outbound_message("events.alpha"),
            )
            .await
            .unwrap()
            .await
            .unwrap_err();
        let commit = publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .message_id("event-3")
                    .header(NATS_BATCH_ID, "batch-1")
                    .header(NATS_BATCH_COMMIT, "1")
                    .payload(Bytes::from_static(b"three"))
                    .outbound_message("events.alpha"),
            )
            .await
            .unwrap()
            .await
            .unwrap();

        assert_eq!(conflict.kind(), PublishErrorKind::WrongLastSequence);
        assert_eq!(commit.sequence, 2);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].payload, Bytes::from_static(b"three"));
    }

    #[tokio::test]
    async fn mock_publish_message_reads_committed_messages() {
        let publisher = MockJetStreamPublishMessage::new();
        publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .message_id("event-1")
                    .payload(Bytes::from_static(b"one"))
                    .outbound_message("events.alpha"),
            )
            .await
            .unwrap()
            .await
            .unwrap();
        publisher
            .publish_message(
                async_nats::jetstream::message::PublishMessage::build()
                    .message_id("event-2")
                    .payload(Bytes::from_static(b"two"))
                    .outbound_message("events.beta"),
            )
            .await
            .unwrap()
            .await
            .unwrap();

        let info = publisher.get_info().await.unwrap();
        let raw = publisher.get_raw_message(1).await.unwrap();
        let latest_alpha = publisher.get_last_raw_message_by_subject("events.alpha").await.unwrap();

        assert_eq!(info.state.last_sequence, 2);
        assert_eq!(raw.payload, Bytes::from_static(b"one"));
        assert_eq!(latest_alpha.sequence, 1);
    }

    #[tokio::test]
    async fn mock_publish_message_missing_reads_return_not_found() {
        let publisher = MockJetStreamPublishMessage::new();

        let latest = publisher
            .get_last_raw_message_by_subject("events.missing")
            .await
            .unwrap_err();
        let raw = publisher.get_raw_message(99).await.unwrap_err();

        assert_eq!(latest.kind(), LastRawMessageErrorKind::NoMessageFound);
        assert_eq!(raw.kind(), LastRawMessageErrorKind::NoMessageFound);
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
    async fn mock_consumer_factory_records_get_stream_calls() {
        let factory = MockJetStreamConsumerFactory::new();

        JetStreamGetStream::get_stream(&factory, "STREAM_A").await.unwrap();
        JetStreamGetStream::get_stream(&factory, "STREAM_B").await.unwrap();

        assert_eq!(factory.get_stream_calls(), vec!["STREAM_A", "STREAM_B"]);
    }

    #[tokio::test]
    async fn mock_stream_returns_last_raw_messages_by_subject() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.add_last_raw_message(StreamMessage {
            subject: "stored.subject".into(),
            sequence: 42,
            headers: HeaderMap::new(),
            payload: Bytes::from_static(br#"{"ok":true}"#),
            time: OffsetDateTime::UNIX_EPOCH,
        });
        let stream = JetStreamGetStream::get_stream(&factory, "stream").await.unwrap();

        let message = stream
            .get_last_raw_message_by_subject("notion.subscription.verification")
            .await
            .unwrap();

        assert_eq!(message.subject.as_str(), "stored.subject");
        assert_eq!(message.sequence, 42);
        assert_eq!(message.payload, Bytes::from_static(br#"{"ok":true}"#));
        assert_eq!(
            factory.last_raw_message_subjects(),
            vec!["notion.subscription.verification"]
        );
    }

    #[tokio::test]
    async fn mock_stream_defaults_to_no_message_found() {
        let factory = MockJetStreamConsumerFactory::new();
        let stream = JetStreamGetStream::get_stream(&factory, "stream").await.unwrap();

        let error = stream.get_last_raw_message_by_subject("subject").await.unwrap_err();

        assert_eq!(error.kind(), LastRawMessageErrorKind::NoMessageFound);
        assert_eq!(error.to_string(), "no message found");
    }

    #[tokio::test]
    async fn mock_stream_returns_configured_last_raw_message_error() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.add_last_raw_message_error(LastRawMessageError::new(LastRawMessageErrorKind::Other));
        let stream = JetStreamGetStream::get_stream(&factory, "stream").await.unwrap();

        let error = stream.get_last_raw_message_by_subject("subject").await.unwrap_err();

        assert_eq!(error.kind(), LastRawMessageErrorKind::Other);
        assert_eq!(error.to_string(), "failed to get last raw message");
    }

    #[tokio::test]
    async fn mock_stream_info_and_raw_messages_are_configurable_from_factory() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.set_info(mock_stream_info(3, 9));
        factory.add_raw_message(
            7,
            StreamMessage {
                subject: "stored.subject".into(),
                sequence: 7,
                headers: HeaderMap::new(),
                payload: Bytes::from_static(b"seven"),
                time: OffsetDateTime::UNIX_EPOCH,
            },
        );
        factory.add_raw_message_error(8, LastRawMessageErrorKind::Other);
        let stream = JetStreamGetStream::get_stream(&factory, "stream").await.unwrap();

        let info = stream.get_info().await.unwrap();
        let message = stream.get_raw_message(7).await.unwrap();
        let configured_error = stream.get_raw_message(8).await.unwrap_err();
        let missing = stream.get_raw_message(9).await.unwrap_err();

        assert_eq!(info.state.messages, 3);
        assert_eq!(info.state.last_sequence, 9);
        assert_eq!(message.payload, Bytes::from_static(b"seven"));
        assert_eq!(configured_error.kind(), LastRawMessageErrorKind::Other);
        assert_eq!(missing.kind(), LastRawMessageErrorKind::NoMessageFound);
        assert_eq!(factory.raw_message_calls(), vec![7, 8, 9]);
    }

    #[tokio::test]
    async fn mock_stream_info_and_raw_messages_are_configurable_from_stream() {
        let factory = MockJetStreamConsumerFactory::new();
        let stream = JetStreamGetStream::get_stream(&factory, "stream").await.unwrap();
        let default_info_error = stream.get_info().await.unwrap_err();
        stream.set_info(mock_stream_info(1, 4));
        stream.add_raw_message(
            4,
            StreamMessage {
                subject: "stored.subject".into(),
                sequence: 4,
                headers: HeaderMap::new(),
                payload: Bytes::from_static(b"four"),
                time: OffsetDateTime::UNIX_EPOCH,
            },
        );
        stream.add_raw_message_error(5, LastRawMessageErrorKind::Other);

        let info = stream.get_info().await.unwrap();
        let message = stream.get_raw_message(4).await.unwrap();
        let error = stream.get_raw_message(5).await.unwrap_err();

        assert_eq!(default_info_error.kind(), RequestErrorKind::Other);
        assert_eq!(info.state.first_sequence, 1);
        assert_eq!(message.sequence, 4);
        assert_eq!(error.kind(), LastRawMessageErrorKind::Other);
        assert_eq!(stream.raw_message_calls(), vec![4, 5]);
    }

    #[tokio::test]
    async fn mock_kv_status_uses_configured_bucket_name() {
        let store = MockJetStreamKvStore::new();
        store.set_bucket_name("custom-bucket");

        let status = store.status().await.unwrap();

        assert_eq!(status.bucket, "custom-bucket");
        assert_eq!(status.info.config.name, "KV_custom-bucket");
    }

    #[tokio::test]
    async fn mock_kv_store_records_core_operations_and_queued_results() {
        let store = MockJetStreamKvStore::new();
        store.enqueue_update_result(Ok(11));
        store.enqueue_update_result(Err(kv::UpdateErrorKind::WrongLastRevision));
        store.enqueue_delete_result(Ok(()));
        store.enqueue_delete_result(Err(kv::DeleteErrorKind::TimedOut));
        store.enqueue_create_result(Ok(12));
        store.enqueue_create_result(Err(kv::CreateErrorKind::AlreadyExists));

        assert_eq!(
            store
                .create_with_ttl("ttl-key", Bytes::from_static(b"ttl"), Duration::from_secs(5))
                .await
                .unwrap(),
            1
        );
        assert_eq!(store.update("key", Bytes::from_static(b"value"), 10).await.unwrap(), 11);
        assert_eq!(
            store
                .update("key", Bytes::from_static(b"value"), 11)
                .await
                .unwrap_err()
                .kind(),
            kv::UpdateErrorKind::WrongLastRevision
        );
        store.delete_expect_revision("key", Some(11)).await.unwrap();
        assert_eq!(
            store.delete_expect_revision("key", None).await.unwrap_err().kind(),
            kv::DeleteErrorKind::TimedOut
        );
        assert_eq!(store.create("new", Bytes::from_static(b"value")).await.unwrap(), 12);
        assert_eq!(
            store
                .create("existing", Bytes::from_static(b"value"))
                .await
                .unwrap_err()
                .kind(),
            kv::CreateErrorKind::AlreadyExists
        );
        assert_eq!(store.create("fallback", Bytes::new()).await.unwrap(), 1);

        assert_eq!(
            store.create_with_ttl_calls(),
            vec![(
                "ttl-key".to_string(),
                Bytes::from_static(b"ttl"),
                Duration::from_secs(5)
            )]
        );
        assert_eq!(
            store.update_calls(),
            vec![
                ("key".to_string(), Bytes::from_static(b"value"), 10),
                ("key".to_string(), Bytes::from_static(b"value"), 11),
            ]
        );
        assert_eq!(
            store.delete_calls(),
            vec![("key".to_string(), Some(11)), ("key".to_string(), None)]
        );
        assert_eq!(
            store.create_calls(),
            vec![
                ("new".to_string(), Bytes::from_static(b"value")),
                ("existing".to_string(), Bytes::from_static(b"value")),
                ("fallback".to_string(), Bytes::new()),
            ]
        );
    }

    #[tokio::test]
    async fn mock_kv_store_reads_entries_gets_and_keys() {
        let store = MockJetStreamKvStore::new();
        store.set_bucket_name("custom-bucket");
        store.enqueue_entry(Bytes::from_static(b"entry"), 21, kv::Operation::Put);
        store.enqueue_entry_none();
        store.enqueue_entry_error(kv::EntryErrorKind::TimedOut);
        store.enqueue_get_some(Bytes::from_static(b"get"));
        store.enqueue_get_none();
        store.enqueue_get_error(kv::EntryErrorKind::Other);
        store.set_keys_result(Ok(vec!["a".to_string(), "b".to_string()]));

        let entry = store.entry("entry-key".to_string()).await.unwrap().unwrap();
        let missing_entry = store.entry("missing-entry".to_string()).await.unwrap();
        let entry_error = store.entry("bad-entry".to_string()).await.unwrap_err();
        let value = store.get("get-key".to_string()).await.unwrap().unwrap();
        let missing_value = store.get("missing-get".to_string()).await.unwrap();
        let get_error = store.get("bad-get".to_string()).await.unwrap_err();
        let keys: Vec<_> = store.keys().await.unwrap().collect().await;

        assert_eq!(entry.bucket, "custom-bucket");
        assert_eq!(entry.key, "entry-key");
        assert_eq!(entry.value, Bytes::from_static(b"entry"));
        assert_eq!(entry.revision, 21);
        assert_eq!(entry.operation, kv::Operation::Put);
        assert!(missing_entry.is_none());
        assert_eq!(entry_error.kind(), kv::EntryErrorKind::TimedOut);
        assert_eq!(value, Bytes::from_static(b"get"));
        assert!(missing_value.is_none());
        assert_eq!(get_error.kind(), kv::EntryErrorKind::Other);
        assert_eq!(keys.into_iter().map(Result::unwrap).collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(store.entry_calls(), vec!["entry-key", "missing-entry", "bad-entry"]);
        assert_eq!(store.get_calls(), vec!["get-key", "missing-get", "bad-get"]);
        assert_eq!(store.keys_calls(), 1);
    }

    #[tokio::test]
    async fn mock_kv_store_surfaces_configured_errors() {
        let store = MockJetStreamKvStore::new();
        store.fail_status(kv::StatusErrorKind::TimedOut);
        store.set_create_with_ttl_result(Err(kv::CreateErrorKind::InvalidKey));
        store.set_update_result(Err(kv::UpdateErrorKind::InvalidKey));
        store.set_delete_result(Err(kv::DeleteErrorKind::InvalidKey));
        store.set_keys_result(Err(kv::WatchErrorKind::ConsumerCreate));

        assert_eq!(store.status().await.unwrap_err().kind(), kv::StatusErrorKind::TimedOut);
        assert_eq!(
            store
                .create_with_ttl("", Bytes::new(), Duration::from_secs(1))
                .await
                .unwrap_err()
                .kind(),
            kv::CreateErrorKind::InvalidKey
        );
        assert_eq!(
            store.update("", Bytes::new(), 1).await.unwrap_err().kind(),
            kv::UpdateErrorKind::InvalidKey
        );
        assert_eq!(
            store.delete_expect_revision("", None).await.unwrap_err().kind(),
            kv::DeleteErrorKind::InvalidKey
        );
        assert_eq!(
            store.keys().await.unwrap_err().kind(),
            kv::WatchErrorKind::ConsumerCreate
        );
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
