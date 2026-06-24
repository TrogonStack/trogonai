//! Stateful in-memory NATS doubles for processor and dispatcher tests.
//!
//! Unlike the enqueue-based mocks, these maintain real per-key state so a test
//! can drive create -> pause -> resume -> remove, observe
//! convergence of the execution schedule, and inject transient backend failures
//! at chosen points.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_nats::HeaderMap;
use async_nats::jetstream::kv;
use async_nats::jetstream::publish::PublishAck;
use async_nats::subject::ToSubject;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::stream::{Iter, iter};
use std::vec::IntoIter;
use time::OffsetDateTime;
use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, Event, EventId, Headers, ReadFrom, ReadStreamRequest,
    ReadStreamResponse, StreamAppend, StreamEvent, StreamPosition, StreamRead, StreamWritePrecondition,
};
use trogon_nats::jetstream::{
    JetStreamKeyValueUpdate, JetStreamKvCreate, JetStreamKvEntry, JetStreamKvGet, JetStreamKvKeys, JetStreamPublisher,
    JetStreamSubjectPurger,
};
use trogonai_proto::scheduler::schedules::v1;
use uuid::Uuid;

use crate::processor::execution::checkpoints::{corrupt_checkpoint_schedule, rewrite_checkpoint_watermark};

use crate::commands::domain::ScheduleId;

/// A faithful in-memory NATS KV bucket with real revisions and injectable
/// transient failures.
#[derive(Clone)]
pub struct InMemoryKv {
    values: Arc<Mutex<HashMap<String, (Bytes, u64)>>>,
    fail_entry: Arc<Mutex<u32>>,
    fail_create: Arc<Mutex<u32>>,
    fail_update: Arc<Mutex<u32>>,
    conflict_update: Arc<Mutex<u32>>,
    miss_entry: Arc<Mutex<u32>>,
    panic_on_entry: Arc<AtomicBool>,
    panic_on_failure_record: Arc<AtomicBool>,
}

impl Default for InMemoryKv {
    fn default() -> Self {
        Self {
            values: Arc::new(Mutex::new(HashMap::new())),
            fail_entry: Arc::new(Mutex::new(0)),
            fail_create: Arc::new(Mutex::new(0)),
            fail_update: Arc::new(Mutex::new(0)),
            conflict_update: Arc::new(Mutex::new(0)),
            miss_entry: Arc::new(Mutex::new(0)),
            panic_on_entry: Arc::new(AtomicBool::new(false)),
            panic_on_failure_record: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl InMemoryKv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fail_next_entry(&self) {
        *self.fail_entry.lock().unwrap() += 1;
    }

    pub fn fail_next_create(&self) {
        *self.fail_create.lock().unwrap() += 1;
    }

    pub fn fail_next_update(&self) {
        *self.fail_update.lock().unwrap() += 1;
    }

    /// Forces the next update to lose a CAS race regardless of revision.
    pub fn conflict_next_update(&self) {
        *self.conflict_update.lock().unwrap() += 1;
    }

    /// Makes the next entry read miss, simulating a load racing a concurrent
    /// first write of the same key.
    pub fn miss_next_entry(&self) {
        *self.miss_entry.lock().unwrap() += 1;
    }

    pub fn panic_on_next_entry(&self) {
        self.panic_on_entry.store(true, Ordering::SeqCst);
    }

    pub fn panic_on_next_failure_record(&self) {
        self.panic_on_failure_record.store(true, Ordering::SeqCst);
    }

    /// Replaces a stored value with corrupt bytes, simulating cache corruption.
    pub fn corrupt(&self, key: &str) {
        let mut values = self.values.lock().unwrap();
        if let Some(entry) = values.get_mut(key) {
            entry.0 = Bytes::from_static(b"not-proto");
        }
    }

    /// Drops the trailing bytes of a stored value, simulating a partially
    /// written blob.
    pub fn truncate_tail(&self, key: &str, dropped: usize) {
        let mut values = self.values.lock().unwrap();
        if let Some(entry) = values.get_mut(key) {
            entry.0 = entry.0.slice(..entry.0.len().saturating_sub(dropped));
        }
    }

    /// Replaces only the embedded schedule snapshot while keeping checkpoint metadata.
    pub fn corrupt_definition(&self, key: &str) {
        let mut values = self.values.lock().unwrap();
        if let Some(entry) = values.get_mut(key) {
            entry.0 = Bytes::from(corrupt_checkpoint_schedule(&entry.0));
        }
    }

    /// Rewrites the checkpoint watermark while keeping the rest of the snapshot intact.
    pub fn inflate_watermark(&self, key: &str, position: u64) {
        let mut values = self.values.lock().unwrap();
        if let Some(entry) = values.get_mut(key) {
            entry.0 = Bytes::from(rewrite_checkpoint_watermark(&entry.0, position));
        }
    }

    /// Drops every stored value, simulating KV bucket loss.
    pub fn clear(&self) {
        self.values.lock().unwrap().clear();
    }

    pub fn contains(&self, key: &str) -> bool {
        self.values.lock().unwrap().contains_key(key)
    }

    fn take(counter: &Arc<Mutex<u32>>) -> bool {
        let mut count = counter.lock().unwrap();
        if *count > 0 {
            *count -= 1;
            true
        } else {
            false
        }
    }
}

impl std::fmt::Debug for InMemoryKv {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("InMemoryKv").finish_non_exhaustive()
    }
}

impl JetStreamKvEntry for InMemoryKv {
    async fn entry(&self, key: String) -> Result<Option<kv::Entry>, kv::EntryError> {
        if self.panic_on_entry.swap(false, Ordering::SeqCst) {
            panic!("simulated checkpoint load panic");
        }
        if Self::take(&self.fail_entry) {
            return Err(kv::EntryError::new(kv::EntryErrorKind::Other));
        }
        if Self::take(&self.miss_entry) {
            return Ok(None);
        }
        let values = self.values.lock().unwrap();
        Ok(values.get(&key).map(|(value, revision)| kv::Entry {
            bucket: "SCHEDULER_SCHEDULE_STATE".to_string(),
            key,
            value: value.clone(),
            revision: *revision,
            delta: 0,
            created: OffsetDateTime::UNIX_EPOCH,
            operation: kv::Operation::Put,
            seen_current: true,
        }))
    }
}

impl JetStreamKvGet for InMemoryKv {
    async fn get(&self, key: String) -> Result<Option<Bytes>, kv::EntryError> {
        let values = self.values.lock().unwrap();
        Ok(values.get(&key).map(|(value, _)| value.clone()))
    }
}

impl JetStreamKvCreate for InMemoryKv {
    async fn create(&self, key: &str, value: Bytes) -> Result<u64, kv::CreateError> {
        if self.panic_on_failure_record.swap(false, Ordering::SeqCst) && key.starts_with("failure.") {
            panic!("simulated failure-record panic");
        }
        if Self::take(&self.fail_create) {
            return Err(kv::CreateError::new(kv::CreateErrorKind::Other));
        }
        let mut values = self.values.lock().unwrap();
        if values.contains_key(key) {
            return Err(kv::CreateError::new(kv::CreateErrorKind::AlreadyExists));
        }
        values.insert(key.to_string(), (value, 1));
        Ok(1)
    }
}

impl JetStreamKeyValueUpdate for InMemoryKv {
    async fn update(&self, key: &str, value: Bytes, revision: u64) -> Result<u64, kv::UpdateError> {
        if Self::take(&self.fail_update) {
            return Err(kv::UpdateError::new(kv::UpdateErrorKind::Other));
        }
        if Self::take(&self.conflict_update) {
            return Err(kv::UpdateError::new(kv::UpdateErrorKind::WrongLastRevision));
        }
        let mut values = self.values.lock().unwrap();
        match values.get_mut(key) {
            Some(entry) if entry.1 == revision => {
                entry.0 = value;
                entry.1 += 1;
                Ok(entry.1)
            }
            _ => Err(kv::UpdateError::new(kv::UpdateErrorKind::WrongLastRevision)),
        }
    }
}

impl JetStreamKvKeys for InMemoryKv {
    type Keys = Iter<IntoIter<Result<String, kv::WatcherError>>>;

    async fn keys(&self) -> Result<Self::Keys, kv::HistoryError> {
        let keys: Vec<Result<String, kv::WatcherError>> = self.values.lock().unwrap().keys().cloned().map(Ok).collect();
        Ok(iter(keys))
    }
}

/// A faithful in-memory execution stream: publish appends, purge clears the
/// subject. Backs both the publisher and the purger so purge-then-publish
/// convergence is observable.
type SubjectMessages = HashMap<String, Vec<(HeaderMap, Bytes)>>;

#[derive(Clone, Default)]
pub struct InMemoryExecution {
    messages: Arc<Mutex<SubjectMessages>>,
    fail_publish: Arc<Mutex<u32>>,
    fail_purge: Arc<Mutex<u32>>,
}

impl InMemoryExecution {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fail_next_publish(&self) {
        *self.fail_publish.lock().unwrap() += 1;
    }

    pub fn fail_next_purge(&self) {
        *self.fail_purge.lock().unwrap() += 1;
    }

    pub fn scheduled_count(&self, subject: &str) -> usize {
        self.messages.lock().unwrap().get(subject).map_or(0, Vec::len)
    }

    pub fn headers_for(&self, subject: &str) -> Option<HeaderMap> {
        self.messages
            .lock()
            .unwrap()
            .get(subject)
            .and_then(|messages| messages.last().map(|(headers, _)| headers.clone()))
    }

    pub fn payload_for(&self, subject: &str) -> Option<Bytes> {
        self.messages
            .lock()
            .unwrap()
            .get(subject)
            .and_then(|messages| messages.last().map(|(_, payload)| payload.clone()))
    }

    fn take(counter: &Arc<Mutex<u32>>) -> bool {
        let mut count = counter.lock().unwrap();
        if *count > 0 {
            *count -= 1;
            true
        } else {
            false
        }
    }
}

impl std::fmt::Debug for InMemoryExecution {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("InMemoryExecution").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct InMemoryError(pub String);

impl JetStreamPublisher for InMemoryExecution {
    type PublishError = InMemoryError;
    type AckFuture = std::future::Ready<Result<PublishAck, InMemoryError>>;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<Self::AckFuture, Self::PublishError> {
        if Self::take(&self.fail_publish) {
            return Err(InMemoryError("simulated publish failure".to_string()));
        }
        let subject = subject.to_subject().to_string();
        self.messages
            .lock()
            .unwrap()
            .entry(subject)
            .or_default()
            .push((headers, payload));
        Ok(std::future::ready(Ok(PublishAck {
            stream: "execution".to_string(),
            sequence: 1,
            domain: String::new(),
            duplicate: false,
            value: None,
        })))
    }
}

impl JetStreamSubjectPurger for InMemoryExecution {
    type PurgeResponse = ();
    type Error = InMemoryError;

    async fn purge_subject_messages(&self, subject: &str) -> Result<Self::PurgeResponse, Self::Error> {
        if Self::take(&self.fail_purge) {
            return Err(InMemoryError("simulated purge failure".to_string()));
        }
        self.messages.lock().unwrap().remove(subject);
        Ok(())
    }
}

/// Builds a persisted `StreamEvent` for a schedule event, mirroring how the
/// durable consumer would deliver it.
pub fn stream_event(event: &v1::ScheduleEvent, schedule_id: &str, position: u64) -> StreamEvent {
    stream_event_with_headers(event, schedule_id, position, Headers::empty())
}

/// Same as [`stream_event`] but with explicit envelope headers (trace context).
pub fn stream_event_with_headers(
    event: &v1::ScheduleEvent,
    schedule_id: &str,
    position: u64,
    headers: Headers,
) -> StreamEvent {
    use trogon_decider_runtime::{EventEncode, EventType};

    let content = event.encode().expect("schedule event encodes");
    let r#type = event.event_type().expect("schedule event has a type").to_string();
    StreamEvent {
        stream_id: schedule_id.to_string(),
        event: Event {
            id: EventId::new(deterministic_event_id(schedule_id, position)),
            r#type,
            content,
            headers,
        },
        stream_position: StreamPosition::try_new(position).expect("position is non-zero"),
        recorded_at: recorded_at(),
    }
}

/// A foreign envelope whose event type is not owned by the scheduler decider.
pub fn foreign_stream_event(position: u64) -> StreamEvent {
    StreamEvent {
        stream_id: "orders/created".to_string(),
        event: Event {
            id: EventId::new(deterministic_event_id("foreign", position)),
            r#type: "trogonai.other.v1.SomethingElse".to_string(),
            content: vec![1, 2, 3],
            headers: Headers::empty(),
        },
        stream_position: StreamPosition::try_new(position).expect("position is non-zero"),
        recorded_at: recorded_at(),
    }
}

/// An envelope that claims to be a scheduler event but carries an undecodable
/// payload, exercising the durable failure-record path.
pub fn malformed_stream_event(position: u64) -> StreamEvent {
    StreamEvent {
        stream_id: "orders/created".to_string(),
        event: Event {
            id: EventId::new(deterministic_event_id("malformed", position)),
            r#type: "trogonai.scheduler.schedules.v1.ScheduleCreated".to_string(),
            content: vec![0xff, 0xff, 0xff, 0xff],
            headers: Headers::empty(),
        },
        stream_position: StreamPosition::try_new(position).expect("position is non-zero"),
        recorded_at: recorded_at(),
    }
}

pub fn schedule_id(raw: &str) -> ScheduleId {
    ScheduleId::parse(raw).unwrap()
}

pub fn recorded_at() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-06-04T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn deterministic_event_id(seed: &str, position: u64) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{seed}:{position}").as_bytes())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MemoryEventStoreError {
    #[error("optimistic concurrency conflict")]
    Conflict,
}

/// In-memory schedule event store with per-stream optimistic concurrency, used
/// to exercise the processor arming the next occurrence via a real command.
#[derive(Debug, Clone, Default)]
pub struct MemoryEventStore {
    streams: Arc<Mutex<HashMap<String, Vec<Event>>>>,
}

impl MemoryEventStore {
    pub fn events(&self, stream_id: &str) -> Vec<StreamEvent> {
        futures::executor::block_on(self.read_stream(ReadStreamRequest {
            stream_id,
            from: ReadFrom::Beginning,
        }))
        .unwrap()
        .events
    }

    fn position_for_len(len: usize) -> Option<StreamPosition> {
        (len > 0).then(|| StreamPosition::try_new(len.try_into().unwrap()).unwrap())
    }
}

impl StreamRead<str> for MemoryEventStore {
    type Error = MemoryEventStoreError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        let streams = self.streams.lock().unwrap();
        let events = streams.get(request.stream_id).cloned().unwrap_or_default();
        let current_position = Self::position_for_len(events.len());
        let from = match request.from {
            ReadFrom::Beginning => 1,
            ReadFrom::Position(position) => position.as_u64(),
        };
        let stream_events = events
            .into_iter()
            .enumerate()
            .filter_map(|(index, event)| {
                let position = u64::try_from(index + 1).unwrap();
                (position >= from).then(|| StreamEvent {
                    stream_id: request.stream_id.to_string(),
                    event,
                    stream_position: StreamPosition::try_new(position).unwrap(),
                    recorded_at: recorded_at(),
                })
            })
            .collect();

        Ok(ReadStreamResponse {
            current_position,
            events: stream_events,
        })
    }
}

impl StreamAppend<str> for MemoryEventStore {
    type Error = MemoryEventStoreError;

    async fn append_stream(&self, request: AppendStreamRequest<'_, str>) -> Result<AppendStreamResponse, Self::Error> {
        let mut streams = self.streams.lock().unwrap();
        let events = streams.entry(request.stream_id.to_string()).or_default();
        let current_position = Self::position_for_len(events.len());
        let precondition_matches = match request.stream_write_precondition {
            StreamWritePrecondition::Any => true,
            StreamWritePrecondition::StreamExists => current_position.is_some(),
            StreamWritePrecondition::NoStream => current_position.is_none(),
            StreamWritePrecondition::At(position) => current_position == Some(position),
        };
        if !precondition_matches {
            return Err(MemoryEventStoreError::Conflict);
        }
        events.extend(request.events);
        let stream_position = StreamPosition::try_new(events.len().try_into().unwrap()).unwrap();

        Ok(AppendStreamResponse { stream_position })
    }
}

#[cfg(test)]
mod tests;
