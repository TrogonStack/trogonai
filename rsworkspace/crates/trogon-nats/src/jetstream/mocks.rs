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
    JetStreamPublisher, JetStreamSubjectPurger,
};
use crate::mocks::MockError;

#[derive(Clone, Debug)]
pub struct MockJetStreamPurger {
    purged_subjects: Arc<Mutex<Vec<String>>>,
    purge_fail_count: Arc<Mutex<u32>>,
}

impl MockJetStreamPurger {
    pub fn new() -> Self {
        Self {
            purged_subjects: Arc::new(Mutex::new(Vec::new())),
            purge_fail_count: Arc::new(Mutex::new(0)),
        }
    }

    pub fn fail_next_purge(&self) {
        *self.purge_fail_count.lock().unwrap() += 1;
    }

    pub fn fail_purge_count(&self, n: u32) {
        *self.purge_fail_count.lock().unwrap() = n;
    }

    pub fn purged_subjects(&self) -> Vec<String> {
        self.purged_subjects.lock().unwrap().clone()
    }
}

impl Default for MockJetStreamPurger {
    fn default() -> Self {
        Self::new()
    }
}

impl JetStreamSubjectPurger for MockJetStreamPurger {
    type PurgeResponse = ();
    type Error = MockError;

    async fn purge_subject_messages(&self, subject: &str) -> Result<Self::PurgeResponse, MockError> {
        let should_fail = {
            let mut count = self.purge_fail_count.lock().unwrap();
            if *count > 0 {
                *count -= 1;
                true
            } else {
                false
            }
        };
        if should_fail {
            return Err(MockError("simulated purge failure".to_string()));
        }
        self.purged_subjects.lock().unwrap().push(subject.to_string());
        Ok(())
    }
}

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
                Ok(futures::stream::iter(items))
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
mod tests;
