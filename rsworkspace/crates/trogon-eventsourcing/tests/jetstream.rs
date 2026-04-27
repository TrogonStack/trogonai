use std::{
    convert::Infallible,
    error::Error,
    num::NonZeroU64,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_nats::{
    HeaderMap,
    header::NATS_MESSAGE_ID,
    jetstream::{self, kv, stream},
};
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use trogon_eventsourcing::list_snapshots;
use trogon_eventsourcing::nats::{
    AppendProjector, JetStreamStore, JetStreamStoreError, NoAppendProjection, StreamSubjectResolver, SubjectState,
    subject_current_version,
};
use trogon_eventsourcing::{
    AppendOutcome, CanonicalEventCodec, CommandExecution, CommandFailure, CommandInfraError, Decide, Decision,
    EventData, EventId, EventIdentity, EventType, FrequencySnapshot, JsonEventCodec, NonEmpty, Snapshot,
    SnapshotChange, SnapshotStoreConfig, Snapshots, StreamState,
};
use trogon_eventsourcing::{
    StreamStoreError, TROGON_EVENT_TYPE, checkpoint_key, load_snapshot_map, maybe_advance_checkpoint,
    persist_snapshot_change, read_checkpoint, snapshot_key, write_checkpoint,
};
use uuid::Uuid;

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
type TestStore<P = NoAppendProjection<std::io::Error>> = JetStreamStore<TestSubjectResolver, P>;

#[derive(Debug, Clone)]
struct TestSubjectResolver {
    prefix: String,
}

impl StreamSubjectResolver<str> for TestSubjectResolver {
    type Error = std::io::Error;

    async fn resolve_subject_state(
        &self,
        events_stream: &jetstream::stream::Stream,
        stream_id: &str,
    ) -> Result<SubjectState, Self::Error> {
        let subject = format!("{}{}", self.prefix, stream_id);
        let current_version = subject_current_version(events_stream, &subject)
            .await
            .map_err(std::io::Error::other)?;
        Ok(SubjectState {
            subject,
            current_version,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TestEvent {
    value: String,
}

impl EventIdentity for TestEvent {}

impl EventType for TestEvent {
    fn event_type(&self) -> &'static str {
        "test.event"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TestSnapshot {
    value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct CounterState {
    total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CounterIncreased {
    amount: u64,
}

impl EventIdentity for CounterIncreased {}

impl EventType for CounterIncreased {
    fn event_type(&self) -> &'static str {
        "test.counter_increased"
    }
}

impl CanonicalEventCodec for CounterIncreased {
    type Codec = JsonEventCodec;

    fn canonical_codec() -> Self::Codec {
        JsonEventCodec
    }
}

#[derive(Debug, Clone)]
struct IncreaseCounterCommand {
    stream_id: String,
    amount: u64,
}

impl IncreaseCounterCommand {
    fn new(stream_id: impl Into<String>, amount: u64) -> Self {
        Self {
            stream_id: stream_id.into(),
            amount,
        }
    }
}

impl Decide for IncreaseCounterCommand {
    type StreamId = str;
    type State = CounterState;
    type Event = CounterIncreased;
    type DecideError = Infallible;
    type EvolveError = Infallible;

    fn stream_id(&self) -> &Self::StreamId {
        &self.stream_id
    }

    fn initial_state() -> Self::State {
        CounterState::default()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        Ok(CounterState {
            total: state.total + event.amount,
        })
    }

    fn decide(_state: &Self::State, command: &Self) -> Result<Decision<Self::Event>, Self::DecideError> {
        Ok(Decision::event(CounterIncreased { amount: command.amount }))
    }
}

#[derive(Debug, Clone, Default)]
struct RecordingProjection {
    projected: Arc<Mutex<Vec<ProjectedAppend>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectedAppend {
    stream_id: String,
    event_count: usize,
    next_expected_version: u64,
}

impl RecordingProjection {
    fn projected(&self) -> TestResult<Vec<ProjectedAppend>> {
        self.projected
            .lock()
            .map(|projected| projected.clone())
            .map_err(|_| std::io::Error::other("projection lock poisoned").into())
    }
}

impl AppendProjector<str> for RecordingProjection {
    type Error = std::io::Error;

    async fn project_appended(
        &self,
        _snapshot_bucket: &kv::Store,
        stream_id: &str,
        events: &[EventData],
        next_expected_version: u64,
    ) -> Result<(), Self::Error> {
        let mut projected = self
            .projected
            .lock()
            .map_err(|_| std::io::Error::other("projection lock poisoned"))?;
        projected.push(ProjectedAppend {
            stream_id: stream_id.to_string(),
            event_count: events.len(),
            next_expected_version,
        });
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct FailingProjection;

impl AppendProjector<str> for FailingProjection {
    type Error = std::io::Error;

    async fn project_appended(
        &self,
        _snapshot_bucket: &kv::Store,
        _stream_id: &str,
        _events: &[EventData],
        _next_expected_version: u64,
    ) -> Result<(), Self::Error> {
        Err(std::io::Error::other("projection failed"))
    }
}

struct JetStreamFixture<P = NoAppendProjection<std::io::Error>> {
    js: jetstream::Context,
    stream_name: String,
    bucket_name: String,
    subject_prefix: String,
    store: TestStore<P>,
}

impl JetStreamFixture<NoAppendProjection<std::io::Error>> {
    async fn new() -> TestResult<Self> {
        Self::new_with_projector(NoAppendProjection::<std::io::Error>::default()).await
    }

    async fn without_atomic_publish() -> TestResult<Self> {
        Self::new_with_projector_and_atomic_publish(NoAppendProjection::<std::io::Error>::default(), false).await
    }
}

impl<P> JetStreamFixture<P>
where
    P: AppendProjector<str, Error = std::io::Error>,
{
    async fn new_with_projector(append_projector: P) -> TestResult<Self> {
        Self::new_with_projector_and_atomic_publish(append_projector, true).await
    }

    async fn new_with_projector_and_atomic_publish(
        append_projector: P,
        allow_atomic_publish: bool,
    ) -> TestResult<Self> {
        let client = tokio::time::timeout(TEST_TIMEOUT, async_nats::connect(nats_test_url()))
            .await
            .map_err(|source| std::io::Error::other(format!("timed out connecting to NATS: {source}")))??;
        let js = jetstream::new(client);
        let suffix = Uuid::new_v4().simple().to_string();
        let stream_name = format!("ESIT_{suffix}");
        let bucket_name = format!("ESIT_SNAP_{suffix}");
        let subject_prefix = format!("esit.{suffix}.");

        let events_stream = js
            .create_stream(stream::Config {
                name: stream_name.clone(),
                subjects: vec![format!("{subject_prefix}>")],
                allow_atomic_publish,
                ..Default::default()
            })
            .await?;
        let snapshot_bucket = js
            .create_key_value(kv::Config {
                bucket: bucket_name.clone(),
                ..Default::default()
            })
            .await?;
        let store = JetStreamStore::new(
            js.clone(),
            events_stream,
            snapshot_bucket,
            TestSubjectResolver {
                prefix: subject_prefix.clone(),
            },
            append_projector,
        );

        Ok(Self {
            js,
            stream_name,
            bucket_name,
            subject_prefix,
            store,
        })
    }

    fn subject(&self, stream_id: &str) -> String {
        format!("{}{}", self.subject_prefix, stream_id)
    }

    async fn delete(self) -> TestResult {
        self.js.delete_stream(self.stream_name).await?;
        self.js.delete_stream(format!("KV_{}", self.bucket_name)).await?;
        Ok(())
    }
}

fn nats_test_url() -> String {
    std::env::var("NATS_TEST_URL").unwrap_or_else(|_| "nats://127.0.0.1:14222".to_string())
}

fn test_event(stream_id: &str, value: impl Into<String>) -> Result<EventData, JetStreamStoreError<std::io::Error>> {
    EventData::new_with_event_id(
        stream_id,
        EventId::from(Uuid::new_v4()),
        TestEvent { value: value.into() },
    )
    .map_err(|source| JetStreamStoreError::Codec(std::io::Error::other(source)))
}

fn test_event_with_id(stream_id: &str, event_id: Uuid, value: impl Into<String>) -> TestResult<EventData> {
    Ok(EventData::new_with_event_id(
        stream_id,
        EventId::from(event_id),
        TestEvent { value: value.into() },
    )?)
}

fn event_batch(events: Vec<EventData>) -> TestResult<NonEmpty<EventData>> {
    NonEmpty::from_vec(events).ok_or_else(|| std::io::Error::other("events must be non-empty").into())
}

fn test_snapshot_config() -> SnapshotStoreConfig {
    SnapshotStoreConfig::without_checkpoint("counter.snapshots.")
}

fn debug_error<E>(error: E) -> std::io::Error
where
    E: std::fmt::Debug,
{
    std::io::Error::other(format!("{error:?}"))
}

async fn append_one<P>(
    store: &TestStore<P>,
    stream_id: &str,
    stream_state: StreamState,
    value: impl Into<String>,
) -> Result<AppendOutcome, JetStreamStoreError<std::io::Error>>
where
    P: AppendProjector<str, Error = std::io::Error>,
{
    store
        .append_to_stream(stream_id, stream_state, NonEmpty::one(test_event(stream_id, value)?))
        .await
}

async fn append_many<P>(
    store: &TestStore<P>,
    stream_id: &str,
    stream_state: StreamState,
    values: Vec<String>,
) -> Result<AppendOutcome, JetStreamStoreError<std::io::Error>>
where
    P: AppendProjector<str, Error = std::io::Error>,
{
    let mut events = Vec::with_capacity(values.len());
    for value in values {
        events.push(test_event(stream_id, value)?);
    }
    let Some(events) = NonEmpty::from_vec(events) else {
        return Err(JetStreamStoreError::Codec(std::io::Error::other(
            "events must be non-empty",
        )));
    };
    store.append_to_stream(stream_id, stream_state, events).await
}

fn assert_occ_conflict(
    result: Result<AppendOutcome, JetStreamStoreError<std::io::Error>>,
    expected: StreamState,
    current_version: Option<u64>,
) -> TestResult {
    match result {
        Err(JetStreamStoreError::OptimisticConcurrencyConflict {
            expected: actual_expected,
            current_version: actual_current_version,
            ..
        }) => {
            assert_eq!(actual_expected, expected);
            assert_eq!(actual_current_version, current_version);
            Ok(())
        }
        other => Err(std::io::Error::other(format!("expected OCC conflict, got {other:?}")).into()),
    }
}

fn assert_append_publish_error(result: Result<AppendOutcome, JetStreamStoreError<std::io::Error>>) -> TestResult {
    match result {
        Err(JetStreamStoreError::AppendStream(StreamStoreError::Publish { .. })) => Ok(()),
        other => Err(std::io::Error::other(format!("expected append publish error, got {other:?}")).into()),
    }
}

fn assert_project_append_error(result: Result<AppendOutcome, JetStreamStoreError<std::io::Error>>) -> TestResult {
    match result {
        Err(JetStreamStoreError::ProjectAppend(_)) => Ok(()),
        other => Err(std::io::Error::other(format!("expected project append error, got {other:?}")).into()),
    }
}

fn assert_read_stream_error<T>(result: Result<T, JetStreamStoreError<std::io::Error>>) -> TestResult
where
    T: std::fmt::Debug,
{
    match result {
        Err(JetStreamStoreError::ReadStream(_)) => Ok(()),
        other => Err(std::io::Error::other(format!("expected stream read error, got {other:?}")).into()),
    }
}

fn assert_snapshot_error<T>(result: Result<T, JetStreamStoreError<std::io::Error>>) -> TestResult
where
    T: std::fmt::Debug,
{
    match result {
        Err(JetStreamStoreError::Snapshot(_)) => Ok(()),
        other => Err(std::io::Error::other(format!("expected snapshot error, got {other:?}")).into()),
    }
}

async fn assert_missing_append_succeeds(stream_state: StreamState) -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let outcome = append_one(&fixture.store, "alpha", stream_state, "created").await?;
    assert_eq!(outcome.next_expected_version, 1);

    let read = fixture.store.read_events_from("alpha", 1).await?;
    assert_eq!(read.current_version, Some(1));
    assert_eq!(read.events.len(), 1);

    fixture.delete().await
}

async fn assert_missing_append_conflicts(stream_state: StreamState) -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let result = append_one(&fixture.store, "alpha", stream_state, "created").await;
    assert_occ_conflict(result, stream_state, None)?;

    let read = fixture.store.read_events_from("alpha", 1).await?;
    assert_eq!(read.current_version, None);
    assert!(read.events.is_empty());

    fixture.delete().await
}

async fn existing_fixture() -> TestResult<(JetStreamFixture, u64)> {
    let fixture = JetStreamFixture::new().await?;
    let outcome = append_one(&fixture.store, "alpha", StreamState::NoStream, "seed").await?;
    assert_eq!(outcome.next_expected_version, 1);

    let read = fixture.store.read_events_from("alpha", 1).await?;
    assert_eq!(read.current_version, Some(1));
    Ok((fixture, 1))
}

async fn assert_existing_append_succeeds(stream_state: StreamState) -> TestResult {
    let (fixture, current_version) = existing_fixture().await?;
    let outcome = append_one(&fixture.store, "alpha", stream_state, "next").await?;
    assert_eq!(outcome.next_expected_version, current_version + 1);

    let read = fixture.store.read_events_from("alpha", 1).await?;
    assert_eq!(read.current_version, Some(current_version + 1));
    assert_eq!(read.events.len(), 2);

    fixture.delete().await
}

async fn assert_existing_append_conflicts(stream_state: StreamState) -> TestResult {
    let (fixture, current_version) = existing_fixture().await?;
    let result = append_one(&fixture.store, "alpha", stream_state, "next").await;
    assert_occ_conflict(result, stream_state, Some(current_version))?;

    let read = fixture.store.read_events_from("alpha", 1).await?;
    assert_eq!(read.current_version, Some(current_version));
    assert_eq!(read.events.len(), 1);

    fixture.delete().await
}

fn missing_event(index: usize) -> std::io::Error {
    std::io::Error::other(format!("missing recorded event at index {index}"))
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_appends_reads_filters_and_preserves_event_envelope() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let first_alpha_id = Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0101);
    let second_alpha_id = Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0102);

    let outcome = fixture
        .store
        .append_to_stream(
            "alpha",
            StreamState::NoStream,
            event_batch(vec![
                test_event_with_id("alpha", first_alpha_id, "alpha-one")?,
                test_event_with_id("alpha", second_alpha_id, "alpha-two")?,
            ])?,
        )
        .await?;
    assert_eq!(outcome.next_expected_version, 2);

    append_one(&fixture.store, "beta", StreamState::NoStream, "beta-one").await?;
    append_one(&fixture.store, "alpha", StreamState::StreamRevision(2), "alpha-three").await?;

    let read = fixture.store.read_events_from("alpha", 1).await?;
    assert_eq!(read.current_version, Some(4));
    assert_eq!(read.events.len(), 3);

    let first = read.events.first().ok_or_else(|| missing_event(0))?;
    let second = read.events.get(1).ok_or_else(|| missing_event(1))?;
    let third = read.events.get(2).ok_or_else(|| missing_event(2))?;

    assert_eq!(first.event_id, EventId::from(first_alpha_id));
    assert_eq!(second.event_id, EventId::from(second_alpha_id));
    assert_eq!(first.event_type, "test.event");
    assert_eq!(first.stream_id(), "alpha");
    assert_eq!(first.recorded_stream_id, format!("{}alpha", fixture.subject_prefix));
    assert_eq!(first.stream_position, None);
    assert_eq!(first.log_position, Some(1));
    assert_eq!(second.log_position, Some(2));
    assert_eq!(third.log_position, Some(4));
    assert_eq!(first.metadata, None);
    assert_eq!(first.decode_data::<TestEvent>()?.value, "alpha-one");
    assert_eq!(second.decode_data::<TestEvent>()?.value, "alpha-two");
    assert_eq!(third.decode_data::<TestEvent>()?.value, "alpha-three");

    let third_log_position = third
        .log_position
        .ok_or_else(|| std::io::Error::other("third event must have a log position"))?;
    let read_from_third = fixture.store.read_events_from("alpha", third_log_position).await?;
    assert_eq!(read_from_third.current_version, Some(third_log_position));
    assert_eq!(read_from_third.events.len(), 1);
    assert_eq!(
        read_from_third.events[0].decode_data::<TestEvent>()?.value,
        "alpha-three"
    );

    let read_after_third = fixture.store.read_events_from("alpha", third_log_position + 1).await?;
    assert_eq!(read_after_third.current_version, Some(third_log_position));
    assert!(read_after_third.events.is_empty());

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_read_ranges_keep_current_version() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    append_one(&fixture.store, "alpha", StreamState::NoStream, "one").await?;
    append_one(&fixture.store, "alpha", StreamState::StreamRevision(1), "two").await?;
    append_one(&fixture.store, "alpha", StreamState::StreamRevision(2), "three").await?;

    let read_from_zero = fixture.store.read_events_from("alpha", 0).await?;
    assert_eq!(read_from_zero.current_version, Some(3));
    assert!(read_from_zero.events.is_empty());

    let read_from_two = fixture.store.read_events_from("alpha", 2).await?;
    assert_eq!(read_from_two.current_version, Some(3));
    assert_eq!(read_from_two.events.len(), 2);
    assert_eq!(read_from_two.events[0].decode_data::<TestEvent>()?.value, "two");
    assert_eq!(read_from_two.events[1].decode_data::<TestEvent>()?.value, "three");

    let read_after_end = fixture.store.read_events_from("alpha", 4).await?;
    assert_eq!(read_after_end.current_version, Some(3));
    assert!(read_after_end.events.is_empty());

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_read_rejects_messages_missing_event_envelope_headers() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        NATS_MESSAGE_ID,
        EventId::from(Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0301)).to_string(),
    );

    fixture
        .store
        .as_jetstream()
        .publish_with_headers(
            fixture.subject("alpha"),
            headers,
            serde_json::to_vec(&TestEvent {
                value: "bad-envelope".to_string(),
            })?
            .into(),
        )
        .await?
        .await?;

    assert_read_stream_error(fixture.store.read_events_from("alpha", 1).await)?;

    let mut headers = HeaderMap::new();
    headers.insert(TROGON_EVENT_TYPE, "test.event");
    fixture
        .store
        .as_jetstream()
        .publish_with_headers(
            fixture.subject("beta"),
            headers,
            serde_json::to_vec(&TestEvent {
                value: "bad-envelope".to_string(),
            })?
            .into(),
        )
        .await?
        .await?;

    assert_read_stream_error(fixture.store.read_events_from("beta", 1).await)?;

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_requires_atomic_publish_stream_support() -> TestResult {
    let fixture = JetStreamFixture::without_atomic_publish().await?;
    let result = append_one(&fixture.store, "alpha", StreamState::NoStream, "created").await;
    assert_append_publish_error(result)?;

    let read = fixture.store.read_events_from("alpha", 1).await?;
    assert_eq!(read.current_version, None);
    assert!(read.events.is_empty());

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_occ_matrix_for_missing_stream() -> TestResult {
    assert_missing_append_succeeds(StreamState::NoStream).await?;
    assert_missing_append_succeeds(StreamState::Any).await?;
    assert_missing_append_succeeds(StreamState::StreamRevision(0)).await?;
    assert_missing_append_conflicts(StreamState::StreamExists).await?;
    assert_missing_append_conflicts(StreamState::StreamRevision(1)).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_occ_matrix_for_existing_stream() -> TestResult {
    assert_existing_append_succeeds(StreamState::Any).await?;
    assert_existing_append_succeeds(StreamState::StreamExists).await?;
    assert_existing_append_succeeds(StreamState::StreamRevision(1)).await?;
    assert_existing_append_conflicts(StreamState::NoStream).await?;
    assert_existing_append_conflicts(StreamState::StreamRevision(0)).await?;
    assert_existing_append_conflicts(StreamState::StreamRevision(2)).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_rejects_unsupported_append_batches() -> TestResult {
    let fixture = JetStreamFixture::new().await?;

    let mixed_streams = fixture
        .store
        .append_to_stream(
            "alpha",
            StreamState::NoStream,
            event_batch(vec![test_event("alpha", "one")?, test_event("beta", "two")?])?,
        )
        .await;
    assert_append_publish_error(mixed_streams)?;

    let metadata_event = EventData::with_metadata_and_event_id(
        "alpha",
        EventId::from(Uuid::new_v4()),
        TestEvent {
            value: "metadata".to_string(),
        },
        Some(TestSnapshot {
            value: "metadata".to_string(),
        }),
    )?;
    let metadata_result = fixture
        .store
        .append_to_stream("alpha", StreamState::NoStream, NonEmpty::one(metadata_event))
        .await;
    assert_append_publish_error(metadata_result)?;

    let read = fixture.store.read_events_from("alpha", 1).await?;
    assert_eq!(read.current_version, None);
    assert!(read.events.is_empty());

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_runs_append_projector_after_successful_append() -> TestResult {
    let projector = RecordingProjection::default();
    let fixture = JetStreamFixture::new_with_projector(projector.clone()).await?;

    let outcome = append_one(&fixture.store, "alpha", StreamState::NoStream, "created").await?;
    assert_eq!(outcome.next_expected_version, 1);
    assert_eq!(
        projector.projected()?,
        vec![ProjectedAppend {
            stream_id: "alpha".to_string(),
            event_count: 1,
            next_expected_version: 1,
        }]
    );

    let conflict = append_one(&fixture.store, "alpha", StreamState::NoStream, "duplicate").await;
    assert_occ_conflict(conflict, StreamState::NoStream, Some(1))?;
    assert_eq!(projector.projected()?.len(), 1);

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_keeps_appended_events_when_projector_fails() -> TestResult {
    let fixture = JetStreamFixture::new_with_projector(FailingProjection).await?;

    let result = append_one(&fixture.store, "alpha", StreamState::NoStream, "created").await;
    assert_project_append_error(result)?;

    let read = fixture.store.read_events_from("alpha", 1).await?;
    assert_eq!(read.current_version, Some(1));
    assert_eq!(read.events.len(), 1);
    assert_eq!(read.events[0].decode_data::<TestEvent>()?.value, "created");

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_executes_commands_with_snapshots() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let snapshot_policy = FrequencySnapshot::new(NonZeroU64::MIN);
    let first_command = IncreaseCounterCommand::new("counter", 2);

    let first = CommandExecution::new(&fixture.store, &first_command)
        .with_snapshot(Snapshots::new(&fixture.store, test_snapshot_config(), snapshot_policy))
        .execute()
        .await
        .map_err(debug_error)?;
    assert_eq!(first.next_expected_version, 1);
    assert_eq!(first.state, CounterState { total: 2 });

    let first_snapshot = fixture
        .store
        .load_snapshot_entry::<_, CounterState>(test_snapshot_config(), "counter")
        .await?
        .ok_or_else(|| std::io::Error::other("counter snapshot should exist"))?;
    assert_eq!(first_snapshot, Snapshot::new(1, CounterState { total: 2 }));

    let second_command = IncreaseCounterCommand::new("counter", 3);
    let second = CommandExecution::new(&fixture.store, &second_command)
        .with_snapshot(Snapshots::new(&fixture.store, test_snapshot_config(), snapshot_policy))
        .execute()
        .await
        .map_err(debug_error)?;
    assert_eq!(second.next_expected_version, 2);
    assert_eq!(second.state, CounterState { total: 5 });

    let second_snapshot = fixture
        .store
        .load_snapshot_entry::<_, CounterState>(test_snapshot_config(), "counter")
        .await?
        .ok_or_else(|| std::io::Error::other("counter snapshot should exist"))?;
    assert_eq!(second_snapshot, Snapshot::new(2, CounterState { total: 5 }));

    let read = fixture.store.read_events_from("counter", 1).await?;
    assert_eq!(read.current_version, Some(2));
    assert_eq!(read.events.len(), 2);
    assert_eq!(read.events[0].decode_data::<CounterIncreased>()?.amount, 2);
    assert_eq!(read.events[1].decode_data::<CounterIncreased>()?.amount, 3);

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_command_execution_reports_decode_errors_from_corrupt_events() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let corrupt_event = EventData {
        event_id: EventId::from(Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0401)),
        event_type: "test.counter_increased".to_string(),
        stream_id: "counter".to_string(),
        payload: b"not-json".to_vec(),
        metadata: None,
    };
    fixture
        .store
        .append_to_stream("counter", StreamState::NoStream, NonEmpty::one(corrupt_event))
        .await?;

    let command = IncreaseCounterCommand::new("counter", 1);
    let result = CommandExecution::new(&fixture.store, &command).execute().await;
    assert!(matches!(
        result,
        Err(CommandFailure::Infra(CommandInfraError::DecodeEvent(_)))
    ));

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_command_execution_rejects_snapshot_ahead_of_stream() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    fixture
        .store
        .save_snapshot_entry(
            test_snapshot_config(),
            "counter",
            Snapshot::new(10, CounterState { total: 10 }),
        )
        .await?;

    let command = IncreaseCounterCommand::new("counter", 1);
    let result = CommandExecution::new(&fixture.store, &command)
        .with_snapshot(Snapshots::new(
            &fixture.store,
            test_snapshot_config(),
            FrequencySnapshot::new(NonZeroU64::MIN),
        ))
        .execute()
        .await;
    assert!(matches!(
        result,
        Err(CommandFailure::Infra(CommandInfraError::SnapshotAheadOfStream {
            snapshot_version: 10,
            stream_version: None,
        }))
    ));

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_allows_only_one_concurrent_no_stream_append() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let attempts = 24;
    let results = join_all((0..attempts).map(|index| {
        let store = fixture.store.clone();
        async move { append_one(&store, "alpha", StreamState::NoStream, format!("attempt-{index}")).await }
    }))
    .await;

    let success_count = results.iter().filter(|result| result.is_ok()).count();
    let conflict_count = results
        .iter()
        .filter(|result| matches!(result, Err(JetStreamStoreError::OptimisticConcurrencyConflict { .. })))
        .count();
    assert_eq!(success_count, 1);
    assert_eq!(conflict_count, attempts - 1);

    let read = fixture.store.read_events_from("alpha", 1).await?;
    assert_eq!(read.current_version, Some(1));
    assert_eq!(read.events.len(), 1);
    assert!(read.events[0].decode_data::<TestEvent>()?.value.starts_with("attempt-"));

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_any_allows_concurrent_appends_without_occ_guard() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let attempts = 24;
    let results = join_all((0..attempts).map(|index| {
        let store = fixture.store.clone();
        async move { append_one(&store, "alpha", StreamState::Any, format!("any-attempt-{index}")).await }
    }))
    .await;

    let success_count = results.iter().filter(|result| result.is_ok()).count();
    assert_eq!(success_count, attempts);

    let read = fixture.store.read_events_from("alpha", 1).await?;
    assert_eq!(read.current_version, Some(attempts as u64));
    assert_eq!(read.events.len(), attempts);

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_any_allows_concurrent_multi_event_batches() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let attempts = 8;
    let events_per_attempt = 2;
    let results = join_all((0..attempts).map(|index| {
        let store = fixture.store.clone();
        async move {
            append_many(
                &store,
                "alpha",
                StreamState::Any,
                vec![format!("batch-{index}-one"), format!("batch-{index}-two")],
            )
            .await
        }
    }))
    .await;

    let success_count = results.iter().filter(|result| result.is_ok()).count();
    assert_eq!(success_count, attempts);

    let read = fixture.store.read_events_from("alpha", 1).await?;
    assert_eq!(read.current_version, Some((attempts * events_per_attempt) as u64));
    assert_eq!(read.events.len(), attempts * events_per_attempt);

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_allows_only_one_concurrent_exact_revision_append() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    append_one(&fixture.store, "alpha", StreamState::NoStream, "seed").await?;
    let attempts = 24;
    let results = join_all((0..attempts).map(|index| {
        let store = fixture.store.clone();
        async move {
            append_one(
                &store,
                "alpha",
                StreamState::StreamRevision(1),
                format!("revision-attempt-{index}"),
            )
            .await
        }
    }))
    .await;

    let success_count = results.iter().filter(|result| result.is_ok()).count();
    let conflict_count = results
        .iter()
        .filter(|result| matches!(result, Err(JetStreamStoreError::OptimisticConcurrencyConflict { .. })))
        .count();
    assert_eq!(success_count, 1);
    assert_eq!(conflict_count, attempts - 1);

    let read = fixture.store.read_events_from("alpha", 1).await?;
    assert_eq!(read.current_version, Some(2));
    assert_eq!(read.events.len(), 2);

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_rejects_duplicate_event_id_without_advancing_stream() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let event_id = Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0201);
    let outcome = fixture
        .store
        .append_to_stream(
            "alpha",
            StreamState::NoStream,
            NonEmpty::one(test_event_with_id("alpha", event_id, "first")?),
        )
        .await?;
    assert_eq!(outcome.next_expected_version, 1);

    let duplicate = fixture
        .store
        .append_to_stream(
            "alpha",
            StreamState::StreamRevision(1),
            NonEmpty::one(test_event_with_id("alpha", event_id, "duplicate")?),
        )
        .await;
    assert_append_publish_error(duplicate)?;

    let read = fixture.store.read_events_from("alpha", 1).await?;
    assert_eq!(read.current_version, Some(1));
    assert_eq!(read.events.len(), 1);
    assert_eq!(read.events[0].decode_data::<TestEvent>()?.value, "first");

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_rejects_duplicate_event_id_inside_atomic_batch_without_partial_write() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let event_id = Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0202);
    fixture
        .store
        .append_to_stream(
            "alpha",
            StreamState::NoStream,
            NonEmpty::one(test_event_with_id("alpha", event_id, "seed")?),
        )
        .await?;

    let duplicate_first = fixture
        .store
        .append_to_stream(
            "alpha",
            StreamState::StreamRevision(1),
            event_batch(vec![
                test_event_with_id("alpha", event_id, "duplicate-first")?,
                test_event("alpha", "new-second")?,
            ])?,
        )
        .await;
    assert_append_publish_error(duplicate_first)?;

    let duplicate_last = fixture
        .store
        .append_to_stream(
            "alpha",
            StreamState::StreamRevision(1),
            event_batch(vec![
                test_event("alpha", "new-first")?,
                test_event_with_id("alpha", event_id, "duplicate-last")?,
            ])?,
        )
        .await;
    assert_append_publish_error(duplicate_last)?;

    let read = fixture.store.read_events_from("alpha", 1).await?;
    assert_eq!(read.current_version, Some(1));
    assert_eq!(read.events.len(), 1);
    assert_eq!(read.events[0].decode_data::<TestEvent>()?.value, "seed");

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_uses_last_subject_sequence_for_occ_after_interleaved_subjects() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    append_one(&fixture.store, "alpha", StreamState::NoStream, "alpha-one").await?;
    append_one(&fixture.store, "beta", StreamState::NoStream, "beta-one").await?;
    append_one(&fixture.store, "alpha", StreamState::StreamRevision(1), "alpha-two").await?;

    let stale_gap_revision = append_one(&fixture.store, "alpha", StreamState::StreamRevision(2), "alpha-stale").await;
    assert_occ_conflict(stale_gap_revision, StreamState::StreamRevision(2), Some(3))?;

    let outcome = append_one(&fixture.store, "alpha", StreamState::StreamRevision(3), "alpha-three").await?;
    assert_eq!(outcome.next_expected_version, 4);

    let read = fixture.store.read_events_from("alpha", 1).await?;
    assert_eq!(read.current_version, Some(4));
    assert_eq!(read.events.len(), 3);
    assert_eq!(read.events[0].log_position, Some(1));
    assert_eq!(read.events[1].log_position, Some(3));
    assert_eq!(read.events[2].log_position, Some(4));

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_rejects_stale_atomic_batch_without_partial_write() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    append_one(&fixture.store, "alpha", StreamState::NoStream, "seed").await?;

    let stale_batch = fixture
        .store
        .append_to_stream(
            "alpha",
            StreamState::StreamRevision(0),
            event_batch(vec![test_event("alpha", "one")?, test_event("alpha", "two")?])?,
        )
        .await;
    assert_occ_conflict(stale_batch, StreamState::StreamRevision(0), Some(1))?;

    let read = fixture.store.read_events_from("alpha", 1).await?;
    assert_eq!(read.current_version, Some(1));
    assert_eq!(read.events.len(), 1);
    assert_eq!(read.events[0].decode_data::<TestEvent>()?.value, "seed");

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_snapshot_store_persists_lists_deletes_and_advances_checkpoint() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let config = SnapshotStoreConfig::with_checkpoint_name("snapshots.v1.", "last_event_sequence");

    let missing: Option<Snapshot<TestSnapshot>> = fixture.store.load_snapshot_entry(config.clone(), "alpha").await?;
    assert_eq!(missing, None);

    fixture
        .store
        .save_snapshot_entry(
            config.clone(),
            "alpha",
            Snapshot::new(
                2,
                TestSnapshot {
                    value: "alpha-v2".to_string(),
                },
            ),
        )
        .await?;
    fixture
        .store
        .save_snapshot_entry(
            config.clone(),
            "alpha",
            Snapshot::new(
                1,
                TestSnapshot {
                    value: "alpha-v1".to_string(),
                },
            ),
        )
        .await?;
    fixture
        .store
        .save_snapshot_entry(
            config.clone(),
            "alpha",
            Snapshot::new(
                3,
                TestSnapshot {
                    value: "alpha-v3".to_string(),
                },
            ),
        )
        .await?;
    fixture
        .store
        .save_snapshot_entry(
            config.clone(),
            "beta",
            Snapshot::new(
                5,
                TestSnapshot {
                    value: "beta-v5".to_string(),
                },
            ),
        )
        .await?;

    let loaded_alpha = fixture
        .store
        .load_snapshot_entry::<_, TestSnapshot>(config.clone(), "alpha")
        .await?
        .ok_or_else(|| std::io::Error::other("alpha snapshot should exist"))?;
    assert_eq!(
        loaded_alpha,
        Snapshot::new(
            3,
            TestSnapshot {
                value: "alpha-v3".to_string(),
            }
        )
    );

    fixture
        .store
        .snapshot_bucket()
        .put("other.namespace", "ignored".into())
        .await?;
    let snapshot_map: std::collections::BTreeMap<String, Snapshot<TestSnapshot>> =
        load_snapshot_map(fixture.store.snapshot_bucket(), &config).await?;
    assert_eq!(snapshot_map.len(), 2);
    assert_eq!(snapshot_map.get("alpha"), Some(&loaded_alpha));
    assert_eq!(
        snapshot_map.get("beta"),
        Some(&Snapshot::new(
            5,
            TestSnapshot {
                value: "beta-v5".to_string(),
            }
        ))
    );

    let snapshots: Vec<Snapshot<TestSnapshot>> = list_snapshots(fixture.store.snapshot_bucket(), &config).await?;
    assert_eq!(snapshots.len(), 2);
    assert!(snapshots.contains(&loaded_alpha));
    assert!(snapshots.contains(&Snapshot::new(
        5,
        TestSnapshot {
            value: "beta-v5".to_string(),
        }
    )));

    persist_snapshot_change::<TestSnapshot>(
        fixture.store.snapshot_bucket(),
        &config,
        SnapshotChange::delete("alpha"),
    )
    .await?;
    let deleted_alpha: Option<Snapshot<TestSnapshot>> =
        fixture.store.load_snapshot_entry(config.clone(), "alpha").await?;
    assert_eq!(deleted_alpha, None);

    fixture
        .store
        .save_snapshot_entry(
            config.clone(),
            "alpha",
            Snapshot::new(
                4,
                TestSnapshot {
                    value: "alpha-v4".to_string(),
                },
            ),
        )
        .await?;
    let recreated_alpha = fixture
        .store
        .load_snapshot_entry::<_, TestSnapshot>(config.clone(), "alpha")
        .await?
        .ok_or_else(|| std::io::Error::other("alpha snapshot should exist after recreate"))?;
    assert_eq!(
        recreated_alpha,
        Snapshot::new(
            4,
            TestSnapshot {
                value: "alpha-v4".to_string(),
            }
        )
    );

    assert_eq!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await?, 0);
    maybe_advance_checkpoint(fixture.store.snapshot_bucket(), &config, 2).await?;
    assert_eq!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await?, 0);
    maybe_advance_checkpoint(fixture.store.snapshot_bucket(), &config, 1).await?;
    assert_eq!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await?, 1);
    maybe_advance_checkpoint(fixture.store.snapshot_bucket(), &config, 3).await?;
    assert_eq!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await?, 1);
    write_checkpoint(fixture.store.snapshot_bucket(), &config, 7).await?;
    assert_eq!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await?, 7);
    fixture.store.snapshot_bucket().delete(checkpoint_key(&config)?).await?;
    assert_eq!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await?, 0);
    maybe_advance_checkpoint(fixture.store.snapshot_bucket(), &config, 1).await?;
    assert_eq!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await?, 1);

    fixture
        .store
        .snapshot_bucket()
        .put(snapshot_key(&config, "corrupt"), "not-json".into())
        .await?;
    assert_snapshot_error(
        fixture
            .store
            .load_snapshot_entry::<_, TestSnapshot>(config.clone(), "corrupt")
            .await,
    )?;

    fixture
        .store
        .snapshot_bucket()
        .put(checkpoint_key(&config)?, "not-a-number".into())
        .await?;
    assert!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await.is_err());

    fixture.delete().await
}
