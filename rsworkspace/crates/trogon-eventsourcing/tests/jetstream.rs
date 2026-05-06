use std::{convert::Infallible, error::Error, num::NonZeroU64, time::Duration};

use async_nats::{
    HeaderMap,
    header::NATS_MESSAGE_ID,
    jetstream::{self, kv, stream},
};
use futures::future::join_all;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use trogon_eventsourcing::list_snapshots;
use trogon_eventsourcing::nats::{
    JetStreamStore, JetStreamStoreError, StreamSubjectResolver, SubjectState, subject_current_version,
};
use trogon_eventsourcing::{
    AppendStreamRequest, AppendStreamResponse, CanonicalEventCodec, CommandExecution, CommandFailure, Decide, Decision,
    EventCodec, EventData, EventId, EventIdentity, EventType, FrequencySnapshot, NonEmpty, ReadSnapshotRequest,
    ReadStreamRequest, Snapshot, SnapshotChange, SnapshotRead, SnapshotStoreConfig, SnapshotWrite, Snapshots,
    StreamAppend, StreamRead, StreamState, WriteSnapshotRequest, spawn_on_tokio,
};
use trogon_eventsourcing::{
    SnapshotStoreError, StreamStoreError, TROGON_EVENT_TYPE, checkpoint_key, maybe_advance_checkpoint,
    persist_snapshot_change, read_checkpoint, read_snapshot_map, snapshot_key, write_checkpoint,
};
use uuid::Uuid;

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
type TestStore = JetStreamStore<TestSubjectResolver>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TestJsonCodec;

impl<T> EventCodec<T> for TestJsonCodec
where
    T: Serialize + DeserializeOwned,
{
    type Error = serde_json::Error;

    fn encode(&self, value: &T) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(value)
    }

    fn decode(&self, _event_type: &str, _stream_id: &str, payload: &[u8]) -> Result<T, Self::Error> {
        serde_json::from_slice(payload)
    }
}

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
    type Error = std::convert::Infallible;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        Ok("test.event")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct IdentifiedTestEvent {
    #[serde(skip)]
    event_id: Option<EventId>,
    value: String,
}

impl EventIdentity for IdentifiedTestEvent {
    fn event_id(&self) -> Option<EventId> {
        self.event_id
    }
}

impl EventType for IdentifiedTestEvent {
    type Error = Infallible;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        Ok("test.event")
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
    type Error = std::convert::Infallible;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        Ok("test.counter_increased")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct IdentifiedCounterIncreased {
    #[serde(skip)]
    event_id: Option<EventId>,
    amount: u64,
}

impl EventIdentity for IdentifiedCounterIncreased {
    fn event_id(&self) -> Option<EventId> {
        self.event_id
    }
}

impl EventType for IdentifiedCounterIncreased {
    type Error = Infallible;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        Ok("test.counter_increased")
    }
}

impl CanonicalEventCodec for CounterIncreased {
    type Codec = TestJsonCodec;

    fn canonical_codec() -> Self::Codec {
        TestJsonCodec
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

struct JetStreamFixture {
    js: jetstream::Context,
    stream_name: String,
    bucket_name: String,
    subject_prefix: String,
    store: TestStore,
}

impl JetStreamFixture {
    async fn new() -> TestResult<Self> {
        Self::new_with_atomic_publish(true).await
    }

    async fn without_atomic_publish() -> TestResult<Self> {
        Self::new_with_atomic_publish(false).await
    }

    async fn new_with_atomic_publish(allow_atomic_publish: bool) -> TestResult<Self> {
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
    let event = TestEvent { value: value.into() };
    EventData::from_event(stream_id, &TestJsonCodec, &event)
        .map_err(|source| JetStreamStoreError::Codec(std::io::Error::other(source)))
}

fn test_event_with_id(stream_id: &str, event_id: Uuid, value: impl Into<String>) -> TestResult<EventData> {
    let event = IdentifiedTestEvent {
        event_id: Some(EventId::from(event_id)),
        value: value.into(),
    };
    Ok(EventData::from_event(stream_id, &TestJsonCodec, &event)?)
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

async fn read_counter_snapshot_until(
    store: &TestStore,
    stream_id: &str,
    expected: Snapshot<CounterState>,
) -> TestResult<Snapshot<CounterState>> {
    let deadline = tokio::time::Instant::now() + TEST_TIMEOUT;

    loop {
        let snapshot = store
            .read_snapshot(ReadSnapshotRequest::new(test_snapshot_config(), stream_id))
            .await?
            .snapshot;

        let snapshot = match snapshot {
            Some(snapshot) if snapshot == expected => return Ok(snapshot),
            snapshot => snapshot,
        };

        if tokio::time::Instant::now() >= deadline {
            return Err(std::io::Error::other(format!(
                "timed out waiting for snapshot {expected:?}, got {snapshot:?}"
            ))
            .into());
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn append_one(
    store: &TestStore,
    stream_id: &str,
    stream_state: StreamState,
    value: impl Into<String>,
) -> Result<AppendStreamResponse, JetStreamStoreError<std::io::Error>> {
    store
        .append_stream(AppendStreamRequest::new(
            stream_id,
            stream_state,
            NonEmpty::one(test_event(stream_id, value)?),
        ))
        .await
}

async fn append_many(
    store: &TestStore,
    stream_id: &str,
    stream_state: StreamState,
    values: Vec<String>,
) -> Result<AppendStreamResponse, JetStreamStoreError<std::io::Error>> {
    let mut events = Vec::with_capacity(values.len());
    for value in values {
        events.push(test_event(stream_id, value)?);
    }
    let Some(events) = NonEmpty::from_vec(events) else {
        return Err(JetStreamStoreError::Codec(std::io::Error::other(
            "events must be non-empty",
        )));
    };
    store
        .append_stream(AppendStreamRequest::new(stream_id, stream_state, events))
        .await
}

fn assert_occ_conflict(
    result: Result<AppendStreamResponse, JetStreamStoreError<std::io::Error>>,
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

fn assert_append_publish_error(
    result: Result<AppendStreamResponse, JetStreamStoreError<std::io::Error>>,
) -> TestResult {
    match result {
        Err(JetStreamStoreError::AppendStream(StreamStoreError::Publish { .. })) => Ok(()),
        other => Err(std::io::Error::other(format!("expected append publish error, got {other:?}")).into()),
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

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(1));
    assert_eq!(read.events.len(), 1);

    fixture.delete().await
}

async fn assert_missing_append_conflicts(stream_state: StreamState) -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let result = append_one(&fixture.store, "alpha", stream_state, "created").await;
    assert_occ_conflict(result, stream_state, None)?;

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, None);
    assert!(read.events.is_empty());

    fixture.delete().await
}

async fn existing_fixture() -> TestResult<(JetStreamFixture, u64)> {
    let fixture = JetStreamFixture::new().await?;
    let outcome = append_one(&fixture.store, "alpha", StreamState::NoStream, "seed").await?;
    assert_eq!(outcome.next_expected_version, 1);

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(1));
    Ok((fixture, 1))
}

async fn assert_existing_append_succeeds(stream_state: StreamState) -> TestResult {
    let (fixture, current_version) = existing_fixture().await?;
    let outcome = append_one(&fixture.store, "alpha", stream_state, "next").await?;
    assert_eq!(outcome.next_expected_version, current_version + 1);

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(current_version + 1));
    assert_eq!(read.events.len(), 2);

    fixture.delete().await
}

async fn assert_existing_append_conflicts(stream_state: StreamState) -> TestResult {
    let (fixture, current_version) = existing_fixture().await?;
    let result = append_one(&fixture.store, "alpha", stream_state, "next").await;
    assert_occ_conflict(result, stream_state, Some(current_version))?;

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
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
        .append_stream(AppendStreamRequest::new(
            "alpha",
            StreamState::NoStream,
            event_batch(vec![
                test_event_with_id("alpha", first_alpha_id, "alpha-one")?,
                test_event_with_id("alpha", second_alpha_id, "alpha-two")?,
            ])?,
        ))
        .await?;
    assert_eq!(outcome.next_expected_version, 2);

    append_one(&fixture.store, "beta", StreamState::NoStream, "beta-one").await?;
    append_one(&fixture.store, "alpha", StreamState::StreamRevision(2), "alpha-three").await?;

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
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
    assert_eq!(
        first.decode_data_with::<TestEvent, _>(&TestJsonCodec)?.value,
        "alpha-one"
    );
    assert_eq!(
        second.decode_data_with::<TestEvent, _>(&TestJsonCodec)?.value,
        "alpha-two"
    );
    assert_eq!(
        third.decode_data_with::<TestEvent, _>(&TestJsonCodec)?.value,
        "alpha-three"
    );

    let third_log_position = third
        .log_position
        .ok_or_else(|| std::io::Error::other("third event must have a log position"))?;
    let read_from_third = fixture
        .store
        .read_stream(ReadStreamRequest::new("alpha", third_log_position))
        .await?;
    assert_eq!(read_from_third.current_version, Some(third_log_position));
    assert_eq!(read_from_third.events.len(), 1);
    assert_eq!(
        read_from_third.events[0]
            .decode_data_with::<TestEvent, _>(&TestJsonCodec)?
            .value,
        "alpha-three"
    );

    let read_after_third = fixture
        .store
        .read_stream(ReadStreamRequest::new("alpha", third_log_position + 1))
        .await?;
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

    let read_from_zero = fixture.store.read_stream(ReadStreamRequest::new("alpha", 0)).await?;
    assert_eq!(read_from_zero.current_version, Some(3));
    assert!(read_from_zero.events.is_empty());

    let read_from_two = fixture.store.read_stream(ReadStreamRequest::new("alpha", 2)).await?;
    assert_eq!(read_from_two.current_version, Some(3));
    assert_eq!(read_from_two.events.len(), 2);
    assert_eq!(
        read_from_two.events[0]
            .decode_data_with::<TestEvent, _>(&TestJsonCodec)?
            .value,
        "two"
    );
    assert_eq!(
        read_from_two.events[1]
            .decode_data_with::<TestEvent, _>(&TestJsonCodec)?
            .value,
        "three"
    );

    let read_after_end = fixture.store.read_stream(ReadStreamRequest::new("alpha", 4)).await?;
    assert_eq!(read_after_end.current_version, Some(3));
    assert!(read_after_end.events.is_empty());

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_reads_subject_suffixes_from_global_offsets() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    append_one(&fixture.store, "alpha", StreamState::NoStream, "alpha-one").await?;
    append_one(&fixture.store, "beta", StreamState::NoStream, "beta-one").await?;
    append_one(&fixture.store, "alpha", StreamState::StreamRevision(1), "alpha-two").await?;
    append_one(&fixture.store, "beta", StreamState::StreamRevision(2), "beta-two").await?;
    append_one(&fixture.store, "alpha", StreamState::StreamRevision(3), "alpha-three").await?;

    let cases = [
        (1, vec![1, 3, 5], vec!["alpha-one", "alpha-two", "alpha-three"]),
        (2, vec![3, 5], vec!["alpha-two", "alpha-three"]),
        (3, vec![3, 5], vec!["alpha-two", "alpha-three"]),
        (4, vec![5], vec!["alpha-three"]),
        (5, vec![5], vec!["alpha-three"]),
        (6, Vec::new(), Vec::new()),
    ];

    for (from_sequence, expected_positions, expected_values) in cases {
        let read = fixture
            .store
            .read_stream(ReadStreamRequest::new("alpha", from_sequence))
            .await?;
        assert_eq!(read.current_version, Some(5));
        assert_eq!(
            read.events.iter().map(|event| event.log_position).collect::<Vec<_>>(),
            expected_positions.into_iter().map(Some).collect::<Vec<_>>()
        );
        let values = read
            .events
            .iter()
            .map(|event| {
                event
                    .decode_data_with::<TestEvent, _>(&TestJsonCodec)
                    .map(|event| event.value)
            })
            .collect::<serde_json::Result<Vec<_>>>()?;
        assert_eq!(values, expected_values);
    }

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_append_responses_match_recorded_log_positions() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let alpha_one = append_one(&fixture.store, "alpha", StreamState::NoStream, "alpha-one").await?;
    let beta_one = append_one(&fixture.store, "beta", StreamState::NoStream, "beta-one").await?;
    let alpha_batch = fixture
        .store
        .append_stream(AppendStreamRequest::new(
            "alpha",
            StreamState::StreamRevision(alpha_one.next_expected_version),
            event_batch(vec![
                test_event("alpha", "alpha-two")?,
                test_event("alpha", "alpha-three")?,
            ])?,
        ))
        .await?;
    let beta_two = append_one(
        &fixture.store,
        "beta",
        StreamState::StreamRevision(beta_one.next_expected_version),
        "beta-two",
    )
    .await?;

    assert_eq!(alpha_one.next_expected_version, 1);
    assert_eq!(beta_one.next_expected_version, 2);
    assert_eq!(alpha_batch.next_expected_version, 4);
    assert_eq!(beta_two.next_expected_version, 5);

    let alpha = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(alpha.current_version, Some(alpha_batch.next_expected_version));
    assert_eq!(
        alpha.events.iter().map(|event| event.log_position).collect::<Vec<_>>(),
        vec![Some(1), Some(3), Some(4)]
    );
    let beta = fixture.store.read_stream(ReadStreamRequest::new("beta", 1)).await?;
    assert_eq!(beta.current_version, Some(beta_two.next_expected_version));
    assert_eq!(
        beta.events.iter().map(|event| event.log_position).collect::<Vec<_>>(),
        vec![Some(2), Some(5)]
    );

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_skips_deleted_messages_inside_read_range() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    append_one(&fixture.store, "alpha", StreamState::NoStream, "one").await?;
    let deleted = append_one(&fixture.store, "alpha", StreamState::StreamRevision(1), "two").await?;
    append_one(&fixture.store, "alpha", StreamState::StreamRevision(2), "three").await?;

    assert!(
        fixture
            .store
            .events_stream()
            .delete_message(deleted.next_expected_version)
            .await?
    );

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(3));
    assert_eq!(read.events.len(), 2);
    assert_eq!(
        read.events[0].decode_data_with::<TestEvent, _>(&TestJsonCodec)?.value,
        "one"
    );
    assert_eq!(
        read.events[1].decode_data_with::<TestEvent, _>(&TestJsonCodec)?.value,
        "three"
    );

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_uses_previous_subject_sequence_after_latest_message_delete() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    append_one(&fixture.store, "alpha", StreamState::NoStream, "one").await?;
    let deleted = append_one(&fixture.store, "alpha", StreamState::StreamRevision(1), "two").await?;
    assert!(
        fixture
            .store
            .events_stream()
            .delete_message(deleted.next_expected_version)
            .await?
    );

    let read_after_delete = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read_after_delete.current_version, Some(1));
    assert_eq!(read_after_delete.events.len(), 1);
    assert_eq!(read_after_delete.events[0].log_position, Some(1));
    assert_eq!(
        read_after_delete.events[0]
            .decode_data_with::<TestEvent, _>(&TestJsonCodec)?
            .value,
        "one"
    );

    let appended = append_one(&fixture.store, "alpha", StreamState::StreamRevision(1), "three").await?;
    assert_eq!(appended.next_expected_version, 3);
    let read_after_append = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read_after_append.current_version, Some(3));
    assert_eq!(read_after_append.events.len(), 2);
    assert_eq!(read_after_append.events[0].log_position, Some(1));
    assert_eq!(read_after_append.events[1].log_position, Some(3));

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

    assert_read_stream_error(fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await)?;

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

    assert_read_stream_error(fixture.store.read_stream(ReadStreamRequest::new("beta", 1)).await)?;

    let mut headers = HeaderMap::new();
    headers.insert(NATS_MESSAGE_ID, "not-a-uuid");
    headers.insert(TROGON_EVENT_TYPE, "test.event");
    fixture
        .store
        .as_jetstream()
        .publish_with_headers(
            fixture.subject("gamma"),
            headers,
            serde_json::to_vec(&TestEvent {
                value: "bad-envelope".to_string(),
            })?
            .into(),
        )
        .await?
        .await?;

    assert_read_stream_error(fixture.store.read_stream(ReadStreamRequest::new("gamma", 1)).await)?;

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_ignores_corrupt_messages_from_other_subjects() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    append_one(&fixture.store, "alpha", StreamState::NoStream, "alpha-one").await?;

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

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(1));
    assert_eq!(read.events.len(), 1);
    assert_eq!(
        read.events[0].decode_data_with::<TestEvent, _>(&TestJsonCodec)?.value,
        "alpha-one"
    );

    assert_read_stream_error(fixture.store.read_stream(ReadStreamRequest::new("beta", 1)).await)?;

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_read_from_sequence_skips_earlier_corrupt_same_subject_messages() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let mut headers = HeaderMap::new();
    headers.insert(TROGON_EVENT_TYPE, "test.event");
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
    let append = append_one(&fixture.store, "alpha", StreamState::Any, "alpha-valid").await?;
    assert_eq!(append.next_expected_version, 2);

    assert_read_stream_error(fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await)?;

    let read_from_valid = fixture.store.read_stream(ReadStreamRequest::new("alpha", 2)).await?;
    assert_eq!(read_from_valid.current_version, Some(2));
    assert_eq!(read_from_valid.events.len(), 1);
    assert_eq!(read_from_valid.events[0].log_position, Some(2));
    assert_eq!(
        read_from_valid.events[0]
            .decode_data_with::<TestEvent, _>(&TestJsonCodec)?
            .value,
        "alpha-valid"
    );

    let read_after_valid = fixture.store.read_stream(ReadStreamRequest::new("alpha", 3)).await?;
    assert_eq!(read_after_valid.current_version, Some(2));
    assert!(read_after_valid.events.is_empty());

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_requires_atomic_publish_stream_support() -> TestResult {
    let fixture = JetStreamFixture::without_atomic_publish().await?;
    let result = append_one(&fixture.store, "alpha", StreamState::NoStream, "created").await;
    assert_append_publish_error(result)?;

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
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
async fn jetstream_store_uses_subject_occ_for_new_streams_after_global_history() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    append_one(&fixture.store, "alpha", StreamState::NoStream, "alpha-one").await?;

    let missing = fixture.store.read_stream(ReadStreamRequest::new("beta", 1)).await?;
    assert_eq!(missing.current_version, None);
    assert!(missing.events.is_empty());

    let beta = append_one(&fixture.store, "beta", StreamState::NoStream, "beta-one").await?;
    assert_eq!(beta.next_expected_version, 2);

    let beta_duplicate = append_one(&fixture.store, "beta", StreamState::NoStream, "beta-duplicate").await;
    assert_occ_conflict(beta_duplicate, StreamState::NoStream, Some(2))?;

    let gamma = append_one(&fixture.store, "gamma", StreamState::StreamRevision(0), "gamma-one").await?;
    assert_eq!(gamma.next_expected_version, 3);

    let beta_read = fixture.store.read_stream(ReadStreamRequest::new("beta", 1)).await?;
    assert_eq!(beta_read.current_version, Some(2));
    assert_eq!(beta_read.events.len(), 1);
    assert_eq!(beta_read.events[0].log_position, Some(2));
    assert_eq!(
        beta_read.events[0]
            .decode_data_with::<TestEvent, _>(&TestJsonCodec)?
            .value,
        "beta-one"
    );

    let gamma_read = fixture.store.read_stream(ReadStreamRequest::new("gamma", 1)).await?;
    assert_eq!(gamma_read.current_version, Some(3));
    assert_eq!(gamma_read.events.len(), 1);
    assert_eq!(gamma_read.events[0].log_position, Some(3));

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_stream_exists_uses_subject_sequence_after_interleaving() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    append_one(&fixture.store, "alpha", StreamState::NoStream, "alpha-one").await?;
    append_one(&fixture.store, "beta", StreamState::NoStream, "beta-one").await?;

    let alpha_two = append_one(&fixture.store, "alpha", StreamState::StreamExists, "alpha-two").await?;
    assert_eq!(alpha_two.next_expected_version, 3);
    append_one(&fixture.store, "beta", StreamState::StreamRevision(2), "beta-two").await?;
    let alpha_three = append_one(&fixture.store, "alpha", StreamState::StreamExists, "alpha-three").await?;
    assert_eq!(alpha_three.next_expected_version, 5);

    let alpha = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(alpha.current_version, Some(5));
    assert_eq!(alpha.events.len(), 3);
    assert_eq!(alpha.events[0].log_position, Some(1));
    assert_eq!(alpha.events[1].log_position, Some(3));
    assert_eq!(alpha.events[2].log_position, Some(5));

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_any_can_start_new_subject_after_global_history() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    append_one(&fixture.store, "alpha", StreamState::NoStream, "alpha-one").await?;
    append_one(&fixture.store, "beta", StreamState::NoStream, "beta-one").await?;

    let gamma = append_one(&fixture.store, "gamma", StreamState::Any, "gamma-one").await?;
    assert_eq!(gamma.next_expected_version, 3);

    let gamma_read = fixture.store.read_stream(ReadStreamRequest::new("gamma", 1)).await?;
    assert_eq!(gamma_read.current_version, Some(3));
    assert_eq!(gamma_read.events.len(), 1);
    assert_eq!(gamma_read.events[0].log_position, Some(3));

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_rejects_unsupported_append_batches() -> TestResult {
    let fixture = JetStreamFixture::new().await?;

    let outside_target_stream = fixture
        .store
        .append_stream(AppendStreamRequest::new(
            "alpha",
            StreamState::NoStream,
            NonEmpty::one(test_event("beta", "wrong-stream")?),
        ))
        .await;
    assert_append_publish_error(outside_target_stream)?;

    let mixed_streams = fixture
        .store
        .append_stream(AppendStreamRequest::new(
            "alpha",
            StreamState::NoStream,
            event_batch(vec![test_event("alpha", "one")?, test_event("beta", "two")?])?,
        ))
        .await;
    assert_append_publish_error(mixed_streams)?;

    let metadata_payload = TestEvent {
        value: "metadata".to_string(),
    };
    let metadata = TestSnapshot {
        value: "metadata".to_string(),
    };
    let metadata_event = EventData::from_event("alpha", &TestJsonCodec, &metadata_payload)?
        .with_metadata(&TestJsonCodec, Some(&metadata))?;
    let metadata_result = fixture
        .store
        .append_stream(AppendStreamRequest::new(
            "alpha",
            StreamState::NoStream,
            NonEmpty::one(metadata_event),
        ))
        .await;
    assert_append_publish_error(metadata_result)?;

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, None);
    assert!(read.events.is_empty());

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_executes_commands_snapshots() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let snapshot_policy = FrequencySnapshot::new(NonZeroU64::MIN);
    let first_command = IncreaseCounterCommand::new("counter", 2);

    let first = CommandExecution::new(&fixture.store, &first_command)
        .with_snapshot(Snapshots::new(&fixture.store, test_snapshot_config(), snapshot_policy))
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .map_err(debug_error)?;
    assert_eq!(first.next_expected_version, 1);
    assert_eq!(first.state, CounterState { total: 2 });

    let first_snapshot =
        read_counter_snapshot_until(&fixture.store, "counter", Snapshot::new(1, CounterState { total: 2 })).await?;
    assert_eq!(first_snapshot, Snapshot::new(1, CounterState { total: 2 }));

    let second_command = IncreaseCounterCommand::new("counter", 3);
    let second = CommandExecution::new(&fixture.store, &second_command)
        .with_snapshot(Snapshots::new(&fixture.store, test_snapshot_config(), snapshot_policy))
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .map_err(debug_error)?;
    assert_eq!(second.next_expected_version, 2);
    assert_eq!(second.state, CounterState { total: 5 });

    let second_snapshot =
        read_counter_snapshot_until(&fixture.store, "counter", Snapshot::new(2, CounterState { total: 5 })).await?;
    assert_eq!(second_snapshot, Snapshot::new(2, CounterState { total: 5 }));

    let read = fixture.store.read_stream(ReadStreamRequest::new("counter", 1)).await?;
    assert_eq!(read.current_version, Some(2));
    assert_eq!(read.events.len(), 2);
    assert_eq!(
        read.events[0]
            .decode_data_with::<CounterIncreased, _>(&TestJsonCodec)?
            .amount,
        2
    );
    assert_eq!(
        read.events[1]
            .decode_data_with::<CounterIncreased, _>(&TestJsonCodec)?
            .amount,
        3
    );

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_command_execution_respects_snapshot_cadence() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let snapshot_policy = FrequencySnapshot::new(
        NonZeroU64::new(2).ok_or_else(|| std::io::Error::other("test cadence must be non-zero"))?,
    );

    let first_command = IncreaseCounterCommand::new("counter", 1);
    let first = CommandExecution::new(&fixture.store, &first_command)
        .with_snapshot(Snapshots::new(&fixture.store, test_snapshot_config(), snapshot_policy))
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .map_err(debug_error)?;
    assert_eq!(first.next_expected_version, 1);
    assert_eq!(first.state, CounterState { total: 1 });
    let first_snapshot: Option<Snapshot<CounterState>> = fixture
        .store
        .read_snapshot(ReadSnapshotRequest::new(test_snapshot_config(), "counter"))
        .await?
        .snapshot;
    assert_eq!(first_snapshot, None);

    let second_command = IncreaseCounterCommand::new("counter", 2);
    let second = CommandExecution::new(&fixture.store, &second_command)
        .with_snapshot(Snapshots::new(&fixture.store, test_snapshot_config(), snapshot_policy))
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .map_err(debug_error)?;
    assert_eq!(second.next_expected_version, 2);
    assert_eq!(second.state, CounterState { total: 3 });
    let second_snapshot =
        read_counter_snapshot_until(&fixture.store, "counter", Snapshot::new(2, CounterState { total: 3 })).await?;
    assert_eq!(second_snapshot, Snapshot::new(2, CounterState { total: 3 }));

    let third_command = IncreaseCounterCommand::new("counter", 3);
    let third = CommandExecution::new(&fixture.store, &third_command)
        .with_snapshot(Snapshots::new(&fixture.store, test_snapshot_config(), snapshot_policy))
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .map_err(debug_error)?;
    assert_eq!(third.next_expected_version, 3);
    assert_eq!(third.state, CounterState { total: 6 });
    let third_snapshot =
        read_counter_snapshot_until(&fixture.store, "counter", Snapshot::new(2, CounterState { total: 3 })).await?;
    assert_eq!(third_snapshot, Snapshot::new(2, CounterState { total: 3 }));

    let fourth_command = IncreaseCounterCommand::new("counter", 4);
    let fourth = CommandExecution::new(&fixture.store, &fourth_command)
        .with_snapshot(Snapshots::new(&fixture.store, test_snapshot_config(), snapshot_policy))
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .map_err(debug_error)?;
    assert_eq!(fourth.next_expected_version, 4);
    assert_eq!(fourth.state, CounterState { total: 10 });
    let fourth_snapshot =
        read_counter_snapshot_until(&fixture.store, "counter", Snapshot::new(4, CounterState { total: 10 })).await?;
    assert_eq!(fourth_snapshot, Snapshot::new(4, CounterState { total: 10 }));

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_command_execution_snapshots_use_log_sequence_after_interleaved_events() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let snapshot_policy = FrequencySnapshot::new(NonZeroU64::MIN);

    let first_command = IncreaseCounterCommand::new("counter", 2);
    let first = CommandExecution::new(&fixture.store, &first_command)
        .with_snapshot(Snapshots::new(&fixture.store, test_snapshot_config(), snapshot_policy))
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .map_err(debug_error)?;
    assert_eq!(first.next_expected_version, 1);
    assert_eq!(first.state, CounterState { total: 2 });

    append_one(&fixture.store, "noise", StreamState::NoStream, "noise-one").await?;

    let second_command = IncreaseCounterCommand::new("counter", 3);
    let second = CommandExecution::new(&fixture.store, &second_command)
        .with_snapshot(Snapshots::new(&fixture.store, test_snapshot_config(), snapshot_policy))
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .map_err(debug_error)?;
    assert_eq!(second.next_expected_version, 3);
    assert_eq!(second.state, CounterState { total: 5 });

    let second_snapshot =
        read_counter_snapshot_until(&fixture.store, "counter", Snapshot::new(3, CounterState { total: 5 })).await?;
    assert_eq!(second_snapshot, Snapshot::new(3, CounterState { total: 5 }));

    let third_command = IncreaseCounterCommand::new("counter", 1);
    let third = CommandExecution::new(&fixture.store, &third_command)
        .with_snapshot(Snapshots::new(&fixture.store, test_snapshot_config(), snapshot_policy))
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .map_err(debug_error)?;
    assert_eq!(third.next_expected_version, 4);
    assert_eq!(third.state, CounterState { total: 6 });

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_command_execution_keeps_interleaved_stream_state_isolated() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let snapshot_policy = FrequencySnapshot::new(NonZeroU64::MIN);

    let alpha_one = IncreaseCounterCommand::new("alpha", 1);
    let alpha_one = CommandExecution::new(&fixture.store, &alpha_one)
        .with_snapshot(Snapshots::new(&fixture.store, test_snapshot_config(), snapshot_policy))
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .map_err(debug_error)?;
    assert_eq!(alpha_one.next_expected_version, 1);
    assert_eq!(alpha_one.state, CounterState { total: 1 });

    let beta_one = IncreaseCounterCommand::new("beta", 10);
    let beta_one = CommandExecution::new(&fixture.store, &beta_one)
        .with_snapshot(Snapshots::new(&fixture.store, test_snapshot_config(), snapshot_policy))
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .map_err(debug_error)?;
    assert_eq!(beta_one.next_expected_version, 2);
    assert_eq!(beta_one.state, CounterState { total: 10 });

    let alpha_two = IncreaseCounterCommand::new("alpha", 2);
    let alpha_two = CommandExecution::new(&fixture.store, &alpha_two)
        .with_snapshot(Snapshots::new(&fixture.store, test_snapshot_config(), snapshot_policy))
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .map_err(debug_error)?;
    assert_eq!(alpha_two.next_expected_version, 3);
    assert_eq!(alpha_two.state, CounterState { total: 3 });

    let beta_two = IncreaseCounterCommand::new("beta", 5);
    let beta_two = CommandExecution::new(&fixture.store, &beta_two)
        .with_snapshot(Snapshots::new(&fixture.store, test_snapshot_config(), snapshot_policy))
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .map_err(debug_error)?;
    assert_eq!(beta_two.next_expected_version, 4);
    assert_eq!(beta_two.state, CounterState { total: 15 });

    let alpha_snapshot =
        read_counter_snapshot_until(&fixture.store, "alpha", Snapshot::new(3, CounterState { total: 3 })).await?;
    assert_eq!(alpha_snapshot, Snapshot::new(3, CounterState { total: 3 }));
    let beta_snapshot =
        read_counter_snapshot_until(&fixture.store, "beta", Snapshot::new(4, CounterState { total: 15 })).await?;
    assert_eq!(beta_snapshot, Snapshot::new(4, CounterState { total: 15 }));

    let alpha = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(alpha.current_version, Some(3));
    assert_eq!(
        alpha.events.iter().map(|event| event.log_position).collect::<Vec<_>>(),
        vec![Some(1), Some(3)]
    );
    let alpha_amounts = alpha
        .events
        .iter()
        .map(|event| {
            event
                .decode_data_with::<CounterIncreased, _>(&TestJsonCodec)
                .map(|event| event.amount)
        })
        .collect::<serde_json::Result<Vec<_>>>()?;
    assert_eq!(alpha_amounts, vec![1, 2]);

    let beta = fixture.store.read_stream(ReadStreamRequest::new("beta", 1)).await?;
    assert_eq!(beta.current_version, Some(4));
    assert_eq!(
        beta.events.iter().map(|event| event.log_position).collect::<Vec<_>>(),
        vec![Some(2), Some(4)]
    );
    let beta_amounts = beta
        .events
        .iter()
        .map(|event| {
            event
                .decode_data_with::<CounterIncreased, _>(&TestJsonCodec)
                .map(|event| event.amount)
        })
        .collect::<serde_json::Result<Vec<_>>>()?;
    assert_eq!(beta_amounts, vec![10, 5]);

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_command_execution_snapshot_skips_earlier_corrupt_same_subject_events() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let mut headers = HeaderMap::new();
    headers.insert(TROGON_EVENT_TYPE, "test.counter_increased");
    fixture
        .store
        .as_jetstream()
        .publish_with_headers(
            fixture.subject("counter"),
            headers,
            serde_json::to_vec(&CounterIncreased { amount: 99 })?.into(),
        )
        .await?
        .await?;
    let seed_payload = IdentifiedCounterIncreased {
        event_id: Some(EventId::from(Uuid::from_u128(
            0x018f_8f4d_94a8_7000_8000_0000_0000_0502,
        ))),
        amount: 5,
    };
    let seed = EventData::from_event("counter", &TestJsonCodec, &seed_payload)?;
    let seed_outcome = fixture
        .store
        .append_stream(AppendStreamRequest::new(
            "counter",
            StreamState::Any,
            NonEmpty::one(seed),
        ))
        .await?;
    assert_eq!(seed_outcome.next_expected_version, 2);

    let command = IncreaseCounterCommand::new("counter", 1);
    let without_snapshot = CommandExecution::new(&fixture.store, &command).execute().await;
    assert!(matches!(without_snapshot, Err(CommandFailure::ReadStream(_))));

    fixture
        .store
        .write_snapshot(WriteSnapshotRequest::new(
            test_snapshot_config(),
            "counter",
            Snapshot::new(2, CounterState { total: 5 }),
        ))
        .await?;
    let snapshot = CommandExecution::new(&fixture.store, &command)
        .with_snapshot(Snapshots::new(
            &fixture.store,
            test_snapshot_config(),
            FrequencySnapshot::new(NonZeroU64::MIN),
        ))
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await
        .map_err(debug_error)?;
    assert_eq!(snapshot.next_expected_version, 3);
    assert_eq!(snapshot.state, CounterState { total: 6 });

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_command_execution_does_not_snapshot_failed_appends() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let first_command = IncreaseCounterCommand::new("counter", 1);
    CommandExecution::new(&fixture.store, &first_command)
        .execute()
        .await
        .map_err(debug_error)?;

    let second_command = IncreaseCounterCommand::new("counter", 1);
    let result = CommandExecution::new(&fixture.store, &second_command)
        .with_write_precondition(StreamState::NoStream)
        .with_snapshot(Snapshots::new(
            &fixture.store,
            test_snapshot_config(),
            FrequencySnapshot::new(NonZeroU64::MIN),
        ))
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await;
    assert!(matches!(
        result,
        Err(CommandFailure::Append(
            JetStreamStoreError::OptimisticConcurrencyConflict { .. }
        ))
    ));

    let snapshot: Option<Snapshot<CounterState>> = fixture
        .store
        .read_snapshot(ReadSnapshotRequest::new(test_snapshot_config(), "counter"))
        .await?
        .snapshot;
    assert_eq!(snapshot, None);
    let read = fixture.store.read_stream(ReadStreamRequest::new("counter", 1)).await?;
    assert_eq!(read.current_version, Some(1));
    assert_eq!(read.events.len(), 1);
    assert_eq!(
        read.events[0]
            .decode_data_with::<CounterIncreased, _>(&TestJsonCodec)?
            .amount,
        1
    );

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_command_execution_ignores_corrupt_events_from_other_subjects() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let mut headers = HeaderMap::new();
    headers.insert(TROGON_EVENT_TYPE, "test.event");
    fixture
        .store
        .as_jetstream()
        .publish_with_headers(
            fixture.subject("noise"),
            headers,
            serde_json::to_vec(&TestEvent {
                value: "bad-envelope".to_string(),
            })?
            .into(),
        )
        .await?
        .await?;

    let command = IncreaseCounterCommand::new("counter", 5);
    let result = CommandExecution::new(&fixture.store, &command)
        .execute()
        .await
        .map_err(debug_error)?;
    assert_eq!(result.next_expected_version, 2);
    assert_eq!(result.state, CounterState { total: 5 });

    let read = fixture.store.read_stream(ReadStreamRequest::new("counter", 1)).await?;
    assert_eq!(read.current_version, Some(2));
    assert_eq!(read.events.len(), 1);
    assert_eq!(read.events[0].log_position, Some(2));
    assert_eq!(
        read.events[0]
            .decode_data_with::<CounterIncreased, _>(&TestJsonCodec)?
            .amount,
        5
    );

    let missing = fixture.store.read_stream(ReadStreamRequest::new("missing", 1)).await?;
    assert_eq!(missing.current_version, None);
    assert!(missing.events.is_empty());

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
        .append_stream(AppendStreamRequest::new(
            "counter",
            StreamState::NoStream,
            NonEmpty::one(corrupt_event),
        ))
        .await?;

    let command = IncreaseCounterCommand::new("counter", 1);
    let result = CommandExecution::new(&fixture.store, &command).execute().await;
    assert!(matches!(result, Err(CommandFailure::DecodeEvent(_))));

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_command_execution_rejects_snapshot_ahead_of_stream() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    fixture
        .store
        .write_snapshot(WriteSnapshotRequest::new(
            test_snapshot_config(),
            "counter",
            Snapshot::new(10, CounterState { total: 10 }),
        ))
        .await?;

    let command = IncreaseCounterCommand::new("counter", 1);
    let result = CommandExecution::new(&fixture.store, &command)
        .with_snapshot(Snapshots::new(
            &fixture.store,
            test_snapshot_config(),
            FrequencySnapshot::new(NonZeroU64::MIN),
        ))
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await;
    assert!(matches!(
        result,
        Err(CommandFailure::SnapshotAheadOfStream {
            snapshot_version: 10,
            stream_version: None,
        })
    ));

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_command_execution_rejects_snapshot_ahead_of_existing_stream() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let command = IncreaseCounterCommand::new("counter", 1);
    CommandExecution::new(&fixture.store, &command)
        .execute()
        .await
        .map_err(debug_error)?;
    fixture
        .store
        .write_snapshot(WriteSnapshotRequest::new(
            test_snapshot_config(),
            "counter",
            Snapshot::new(10, CounterState { total: 10 }),
        ))
        .await?;

    let command = IncreaseCounterCommand::new("counter", 1);
    let result = CommandExecution::new(&fixture.store, &command)
        .with_snapshot(Snapshots::new(
            &fixture.store,
            test_snapshot_config(),
            FrequencySnapshot::new(NonZeroU64::MIN),
        ))
        .with_task_runtime(spawn_on_tokio)
        .execute()
        .await;
    assert!(matches!(
        result,
        Err(CommandFailure::SnapshotAheadOfStream {
            snapshot_version: 10,
            stream_version: Some(1),
        })
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

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(1));
    assert_eq!(read.events.len(), 1);
    assert!(
        read.events[0]
            .decode_data_with::<TestEvent, _>(&TestJsonCodec)?
            .value
            .starts_with("attempt-")
    );

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_allows_only_one_concurrent_stream_revision_zero_append() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let attempts = 24;
    let results = join_all((0..attempts).map(|index| {
        let store = fixture.store.clone();
        async move {
            append_one(
                &store,
                "alpha",
                StreamState::StreamRevision(0),
                format!("revision-zero-attempt-{index}"),
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

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(1));
    assert_eq!(read.events.len(), 1);

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_allows_only_one_concurrent_no_stream_batch_append() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let attempts = 12;
    let events_per_attempt = 2;
    let results = join_all((0..attempts).map(|index| {
        let store = fixture.store.clone();
        async move {
            append_many(
                &store,
                "alpha",
                StreamState::NoStream,
                vec![format!("batch-{index}-one"), format!("batch-{index}-two")],
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

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(events_per_attempt));
    assert_eq!(read.events.len(), events_per_attempt as usize);

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

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(attempts as u64));
    assert_eq!(read.events.len(), attempts);

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_any_concurrent_appends_preserve_every_event_once() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let attempts = 16;
    let results = join_all((0..attempts).map(|index| {
        let store = fixture.store.clone();
        async move { append_one(&store, "alpha", StreamState::Any, format!("any-{index}")).await }
    }))
    .await;
    for result in results {
        result?;
    }

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(attempts as u64));
    assert_eq!(
        read.events.iter().map(|event| event.log_position).collect::<Vec<_>>(),
        (1..=attempts as u64).map(Some).collect::<Vec<_>>()
    );
    let mut values = read
        .events
        .iter()
        .map(|event| {
            event
                .decode_data_with::<TestEvent, _>(&TestJsonCodec)
                .map(|event| event.value)
        })
        .collect::<serde_json::Result<Vec<_>>>()?;
    values.sort();
    let mut expected_values = (0..attempts).map(|index| format!("any-{index}")).collect::<Vec<_>>();
    expected_values.sort();
    assert_eq!(values, expected_values);

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

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some((attempts * events_per_attempt) as u64));
    assert_eq!(read.events.len(), attempts * events_per_attempt);

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_any_rejects_concurrent_duplicate_event_ids_without_advancing_stream() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let attempts = 12;
    let event_id = Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0206);
    let results = join_all((0..attempts).map(|index| {
        let store = fixture.store.clone();
        async move {
            let payload = IdentifiedTestEvent {
                event_id: Some(EventId::from(event_id)),
                value: format!("attempt-{index}"),
            };
            let event = EventData::from_event("alpha", &TestJsonCodec, &payload)
                .map_err(|source| JetStreamStoreError::Codec(std::io::Error::other(source)))?;
            store
                .append_stream(AppendStreamRequest::new(
                    "alpha",
                    StreamState::Any,
                    NonEmpty::one(event),
                ))
                .await
        }
    }))
    .await;

    let success_count = results.iter().filter(|result| result.is_ok()).count();
    let publish_error_count = results
        .iter()
        .filter(|result| {
            matches!(
                result,
                Err(JetStreamStoreError::AppendStream(StreamStoreError::Publish { .. }))
            )
        })
        .count();
    assert_eq!(success_count, 1);
    assert_eq!(publish_error_count, attempts - 1);

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(1));
    assert_eq!(read.events.len(), 1);
    assert_eq!(read.events[0].event_id, EventId::from(event_id));

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

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(2));
    assert_eq!(read.events.len(), 2);

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_allows_only_one_concurrent_exact_revision_batch_append() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    append_one(&fixture.store, "alpha", StreamState::NoStream, "seed").await?;
    let attempts = 12;
    let events_per_attempt = 2;
    let results = join_all((0..attempts).map(|index| {
        let store = fixture.store.clone();
        async move {
            append_many(
                &store,
                "alpha",
                StreamState::StreamRevision(1),
                vec![
                    format!("revision-batch-{index}-one"),
                    format!("revision-batch-{index}-two"),
                ],
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

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(1 + events_per_attempt));
    assert_eq!(read.events.len(), 1 + events_per_attempt as usize);

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_allows_concurrent_stream_exists_appends() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    append_one(&fixture.store, "alpha", StreamState::NoStream, "seed").await?;
    let attempts = 24;
    let results = join_all((0..attempts).map(|index| {
        let store = fixture.store.clone();
        async move {
            append_one(
                &store,
                "alpha",
                StreamState::StreamExists,
                format!("exists-attempt-{index}"),
            )
            .await
        }
    }))
    .await;

    let success_count = results.iter().filter(|result| result.is_ok()).count();
    assert_eq!(success_count, attempts);

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.events.len(), 1 + attempts);
    assert_eq!(
        read.current_version,
        read.events.last().and_then(|event| event.log_position)
    );

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_allows_concurrent_stream_exists_batch_appends() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    append_one(&fixture.store, "alpha", StreamState::NoStream, "seed").await?;
    let attempts = 12;
    let events_per_attempt = 2;
    let results = join_all((0..attempts).map(|index| {
        let store = fixture.store.clone();
        async move {
            append_many(
                &store,
                "alpha",
                StreamState::StreamExists,
                vec![format!("exists-batch-{index}-one"), format!("exists-batch-{index}-two")],
            )
            .await
        }
    }))
    .await;

    let success_count = results.iter().filter(|result| result.is_ok()).count();
    assert_eq!(success_count, attempts);

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.events.len(), 1 + attempts * events_per_attempt);
    assert_eq!(
        read.current_version,
        read.events.last().and_then(|event| event.log_position)
    );

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_rejects_duplicate_event_id_without_advancing_stream() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let event_id = Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0201);
    let outcome = fixture
        .store
        .append_stream(AppendStreamRequest::new(
            "alpha",
            StreamState::NoStream,
            NonEmpty::one(test_event_with_id("alpha", event_id, "first")?),
        ))
        .await?;
    assert_eq!(outcome.next_expected_version, 1);

    let duplicate = fixture
        .store
        .append_stream(AppendStreamRequest::new(
            "alpha",
            StreamState::StreamRevision(1),
            NonEmpty::one(test_event_with_id("alpha", event_id, "duplicate")?),
        ))
        .await;
    assert_append_publish_error(duplicate)?;

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(1));
    assert_eq!(read.events.len(), 1);
    assert_eq!(
        read.events[0].decode_data_with::<TestEvent, _>(&TestJsonCodec)?.value,
        "first"
    );

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_rejects_duplicate_event_id_with_any_without_advancing_stream() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let event_id = Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0205);
    fixture
        .store
        .append_stream(AppendStreamRequest::new(
            "alpha",
            StreamState::NoStream,
            NonEmpty::one(test_event_with_id("alpha", event_id, "first")?),
        ))
        .await?;

    let duplicate = fixture
        .store
        .append_stream(AppendStreamRequest::new(
            "alpha",
            StreamState::Any,
            NonEmpty::one(test_event_with_id("alpha", event_id, "duplicate")?),
        ))
        .await;
    assert_append_publish_error(duplicate)?;

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(1));
    assert_eq!(read.events.len(), 1);
    assert_eq!(
        read.events[0].decode_data_with::<TestEvent, _>(&TestJsonCodec)?.value,
        "first"
    );

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_rejects_duplicate_event_id_inside_atomic_batch_without_partial_write() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let event_id = Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0202);
    fixture
        .store
        .append_stream(AppendStreamRequest::new(
            "alpha",
            StreamState::NoStream,
            NonEmpty::one(test_event_with_id("alpha", event_id, "seed")?),
        ))
        .await?;

    let duplicate_first = fixture
        .store
        .append_stream(AppendStreamRequest::new(
            "alpha",
            StreamState::StreamRevision(1),
            event_batch(vec![
                test_event_with_id("alpha", event_id, "duplicate-first")?,
                test_event("alpha", "new-second")?,
            ])?,
        ))
        .await;
    assert_append_publish_error(duplicate_first)?;

    let duplicate_last = fixture
        .store
        .append_stream(AppendStreamRequest::new(
            "alpha",
            StreamState::StreamRevision(1),
            event_batch(vec![
                test_event("alpha", "new-first")?,
                test_event_with_id("alpha", event_id, "duplicate-last")?,
            ])?,
        ))
        .await;
    assert_append_publish_error(duplicate_last)?;

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(1));
    assert_eq!(read.events.len(), 1);
    assert_eq!(
        read.events[0].decode_data_with::<TestEvent, _>(&TestJsonCodec)?.value,
        "seed"
    );

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_rejects_duplicate_event_id_inside_new_atomic_batch_without_partial_write() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let event_id = Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0203);

    let duplicate_batch = fixture
        .store
        .append_stream(AppendStreamRequest::new(
            "alpha",
            StreamState::NoStream,
            event_batch(vec![
                test_event_with_id("alpha", event_id, "first")?,
                test_event_with_id("alpha", event_id, "second")?,
            ])?,
        ))
        .await;
    assert_append_publish_error(duplicate_batch)?;

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, None);
    assert!(read.events.is_empty());

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_rejects_duplicate_event_id_across_streams_without_partial_write() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let event_id = Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0204);

    fixture
        .store
        .append_stream(AppendStreamRequest::new(
            "alpha",
            StreamState::NoStream,
            NonEmpty::one(test_event_with_id("alpha", event_id, "alpha")?),
        ))
        .await?;
    let duplicate = fixture
        .store
        .append_stream(AppendStreamRequest::new(
            "beta",
            StreamState::NoStream,
            NonEmpty::one(test_event_with_id("beta", event_id, "beta")?),
        ))
        .await;
    assert_append_publish_error(duplicate)?;

    let alpha = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(alpha.current_version, Some(1));
    assert_eq!(alpha.events.len(), 1);
    let beta = fixture.store.read_stream(ReadStreamRequest::new("beta", 1)).await?;
    assert_eq!(beta.current_version, None);
    assert!(beta.events.is_empty());

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_uses_last_subject_sequence_for_occ_after_interleaved_subjects() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    append_one(&fixture.store, "alpha", StreamState::NoStream, "alpha-one").await?;
    append_one(&fixture.store, "beta", StreamState::NoStream, "beta-one").await?;
    let interleaved_outcome = append_one(&fixture.store, "alpha", StreamState::StreamRevision(1), "alpha-two").await?;
    assert_eq!(interleaved_outcome.next_expected_version, 3);

    let stale_gap_revision = append_one(&fixture.store, "alpha", StreamState::StreamRevision(2), "alpha-stale").await;
    assert_occ_conflict(stale_gap_revision, StreamState::StreamRevision(2), Some(3))?;

    let outcome = append_one(&fixture.store, "alpha", StreamState::StreamRevision(3), "alpha-three").await?;
    assert_eq!(outcome.next_expected_version, 4);

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(4));
    assert_eq!(read.events.len(), 3);
    assert_eq!(read.events[0].log_position, Some(1));
    assert_eq!(read.events[1].log_position, Some(3));
    assert_eq!(read.events[2].log_position, Some(4));

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_store_returns_batch_commit_sequence_after_interleaved_subjects() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    append_one(&fixture.store, "alpha", StreamState::NoStream, "alpha-one").await?;
    append_one(&fixture.store, "beta", StreamState::NoStream, "beta-one").await?;

    let outcome = fixture
        .store
        .append_stream(AppendStreamRequest::new(
            "alpha",
            StreamState::StreamRevision(1),
            event_batch(vec![
                test_event("alpha", "alpha-two")?,
                test_event("alpha", "alpha-three")?,
            ])?,
        ))
        .await?;
    assert_eq!(outcome.next_expected_version, 4);

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
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
        .append_stream(AppendStreamRequest::new(
            "alpha",
            StreamState::StreamRevision(0),
            event_batch(vec![test_event("alpha", "one")?, test_event("alpha", "two")?])?,
        ))
        .await;
    assert_occ_conflict(stale_batch, StreamState::StreamRevision(0), Some(1))?;

    let read = fixture.store.read_stream(ReadStreamRequest::new("alpha", 1)).await?;
    assert_eq!(read.current_version, Some(1));
    assert_eq!(read.events.len(), 1);
    assert_eq!(
        read.events[0].decode_data_with::<TestEvent, _>(&TestJsonCodec)?.value,
        "seed"
    );

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_snapshot_store_persists_lists_deletes_and_advances_checkpoint() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let config = SnapshotStoreConfig::with_checkpoint_name("snapshots.v1.", "last_event_sequence");

    let missing: Option<Snapshot<TestSnapshot>> = fixture
        .store
        .read_snapshot(ReadSnapshotRequest::new(config.clone(), "alpha"))
        .await?
        .snapshot;
    assert_eq!(missing, None);

    fixture
        .store
        .write_snapshot(WriteSnapshotRequest::new(
            config.clone(),
            "alpha",
            Snapshot::new(
                2,
                TestSnapshot {
                    value: "alpha-v2".to_string(),
                },
            ),
        ))
        .await?;
    fixture
        .store
        .write_snapshot(WriteSnapshotRequest::new(
            config.clone(),
            "alpha",
            Snapshot::new(
                1,
                TestSnapshot {
                    value: "alpha-v1".to_string(),
                },
            ),
        ))
        .await?;
    fixture
        .store
        .write_snapshot(WriteSnapshotRequest::new(
            config.clone(),
            "alpha",
            Snapshot::new(
                3,
                TestSnapshot {
                    value: "alpha-v3".to_string(),
                },
            ),
        ))
        .await?;
    fixture
        .store
        .write_snapshot(WriteSnapshotRequest::new(
            config.clone(),
            "beta",
            Snapshot::new(
                5,
                TestSnapshot {
                    value: "beta-v5".to_string(),
                },
            ),
        ))
        .await?;

    let loaded_alpha = fixture
        .store
        .read_snapshot(ReadSnapshotRequest::new(config.clone(), "alpha"))
        .await?
        .snapshot
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
        read_snapshot_map(fixture.store.snapshot_bucket(), &config).await?;
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
    let deleted_alpha: Option<Snapshot<TestSnapshot>> = fixture
        .store
        .read_snapshot(ReadSnapshotRequest::new(config.clone(), "alpha"))
        .await?
        .snapshot;
    assert_eq!(deleted_alpha, None);

    let snapshot_map_after_delete: std::collections::BTreeMap<String, Snapshot<TestSnapshot>> =
        read_snapshot_map(fixture.store.snapshot_bucket(), &config).await?;
    assert_eq!(snapshot_map_after_delete.len(), 1);
    assert!(!snapshot_map_after_delete.contains_key("alpha"));
    assert!(snapshot_map_after_delete.contains_key("beta"));

    let snapshots_after_delete: Vec<Snapshot<TestSnapshot>> =
        list_snapshots(fixture.store.snapshot_bucket(), &config).await?;
    assert_eq!(snapshots_after_delete.len(), 1);
    assert_eq!(
        snapshots_after_delete[0],
        Snapshot::new(
            5,
            TestSnapshot {
                value: "beta-v5".to_string(),
            }
        )
    );

    fixture
        .store
        .write_snapshot(WriteSnapshotRequest::new(
            config.clone(),
            "alpha",
            Snapshot::new(
                4,
                TestSnapshot {
                    value: "alpha-v4".to_string(),
                },
            ),
        ))
        .await?;
    let recreated_alpha = fixture
        .store
        .read_snapshot(ReadSnapshotRequest::new(config.clone(), "alpha"))
        .await?
        .snapshot
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
        SnapshotRead::<TestSnapshot, str>::read_snapshot(
            &fixture.store,
            ReadSnapshotRequest::new(config.clone(), "corrupt"),
        )
        .await,
    )?;
    assert_snapshot_error(
        fixture
            .store
            .write_snapshot(WriteSnapshotRequest::new(
                config.clone(),
                "corrupt",
                Snapshot::new(
                    1,
                    TestSnapshot {
                        value: "still-corrupt".to_string(),
                    },
                ),
            ))
            .await,
    )?;
    persist_snapshot_change::<TestSnapshot>(
        fixture.store.snapshot_bucket(),
        &config,
        SnapshotChange::delete("corrupt"),
    )
    .await?;
    let deleted_corrupt: Option<Snapshot<TestSnapshot>> = fixture
        .store
        .read_snapshot(ReadSnapshotRequest::new(config.clone(), "corrupt"))
        .await?
        .snapshot;
    assert_eq!(deleted_corrupt, None);
    fixture
        .store
        .write_snapshot(WriteSnapshotRequest::new(
            config.clone(),
            "corrupt",
            Snapshot::new(
                2,
                TestSnapshot {
                    value: "recreated-corrupt-key".to_string(),
                },
            ),
        ))
        .await?;
    let recreated_corrupt = fixture
        .store
        .read_snapshot(ReadSnapshotRequest::new(config.clone(), "corrupt"))
        .await?
        .snapshot
        .ok_or_else(|| std::io::Error::other("corrupt snapshot key should be recreated"))?;
    assert_eq!(
        recreated_corrupt,
        Snapshot::new(
            2,
            TestSnapshot {
                value: "recreated-corrupt-key".to_string(),
            }
        )
    );

    fixture
        .store
        .snapshot_bucket()
        .put(checkpoint_key(&config)?, "not-a-number".into())
        .await?;
    assert!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await.is_err());
    write_checkpoint(fixture.store.snapshot_bucket(), &config, 8).await?;
    assert_eq!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await?, 8);

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_snapshot_store_concurrent_upserts_keep_highest_version() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let config = SnapshotStoreConfig::without_checkpoint("snapshots.concurrent.");
    let results = join_all((1..=32).map(|version| {
        let store = fixture.store.clone();
        let config = config.clone();
        async move {
            store
                .write_snapshot(WriteSnapshotRequest::new(
                    config,
                    "alpha",
                    Snapshot::new(
                        version,
                        TestSnapshot {
                            value: format!("alpha-v{version}"),
                        },
                    ),
                ))
                .await
        }
    }))
    .await;

    for result in results {
        result?;
    }

    let loaded = fixture
        .store
        .read_snapshot(ReadSnapshotRequest::new(config, "alpha"))
        .await?
        .snapshot
        .ok_or_else(|| std::io::Error::other("alpha snapshot should exist"))?;
    assert_eq!(
        loaded,
        Snapshot::new(
            32,
            TestSnapshot {
                value: "alpha-v32".to_string(),
            }
        )
    );

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_snapshot_store_map_and_list_agree_after_mixed_changes() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let config = SnapshotStoreConfig::without_checkpoint("snapshots.consistency.");

    fixture
        .store
        .write_snapshot(WriteSnapshotRequest::new(
            config.clone(),
            "alpha",
            Snapshot::new(
                1,
                TestSnapshot {
                    value: "alpha-v1".to_string(),
                },
            ),
        ))
        .await?;
    fixture
        .store
        .write_snapshot(WriteSnapshotRequest::new(
            config.clone(),
            "alpha",
            Snapshot::new(
                2,
                TestSnapshot {
                    value: "alpha-v2".to_string(),
                },
            ),
        ))
        .await?;
    fixture
        .store
        .write_snapshot(WriteSnapshotRequest::new(
            config.clone(),
            "beta",
            Snapshot::new(
                3,
                TestSnapshot {
                    value: "beta-v3".to_string(),
                },
            ),
        ))
        .await?;
    fixture
        .store
        .write_snapshot(WriteSnapshotRequest::new(
            config.clone(),
            "gamma",
            Snapshot::new(
                4,
                TestSnapshot {
                    value: "gamma-v4".to_string(),
                },
            ),
        ))
        .await?;
    persist_snapshot_change::<TestSnapshot>(
        fixture.store.snapshot_bucket(),
        &config,
        SnapshotChange::delete("gamma"),
    )
    .await?;

    let snapshot_map: std::collections::BTreeMap<String, Snapshot<TestSnapshot>> =
        read_snapshot_map(fixture.store.snapshot_bucket(), &config).await?;
    assert_eq!(snapshot_map.len(), 2);
    assert_eq!(
        snapshot_map.get("alpha"),
        Some(&Snapshot::new(
            2,
            TestSnapshot {
                value: "alpha-v2".to_string(),
            }
        ))
    );
    assert_eq!(
        snapshot_map.get("beta"),
        Some(&Snapshot::new(
            3,
            TestSnapshot {
                value: "beta-v3".to_string(),
            }
        ))
    );
    assert!(!snapshot_map.contains_key("gamma"));

    let mut listed = list_snapshots::<TestSnapshot>(fixture.store.snapshot_bucket(), &config)
        .await?
        .into_iter()
        .map(|snapshot| (snapshot.version, snapshot.payload.value))
        .collect::<Vec<_>>();
    listed.sort();
    assert_eq!(listed, vec![(2, "alpha-v2".to_string()), (3, "beta-v3".to_string())]);

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_snapshot_store_concurrent_deletes_are_idempotent() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let config = SnapshotStoreConfig::without_checkpoint("snapshots.delete.");
    fixture
        .store
        .write_snapshot(WriteSnapshotRequest::new(
            config.clone(),
            "alpha",
            Snapshot::new(
                1,
                TestSnapshot {
                    value: "alpha-v1".to_string(),
                },
            ),
        ))
        .await?;

    let results =
        join_all(
            (0..24).map(|_| {
                let bucket = fixture.store.snapshot_bucket().clone();
                let config = config.clone();
                async move {
                    persist_snapshot_change::<TestSnapshot>(&bucket, &config, SnapshotChange::delete("alpha")).await
                }
            }),
        )
        .await;
    for result in results {
        result?;
    }

    let deleted_alpha: Option<Snapshot<TestSnapshot>> = fixture
        .store
        .read_snapshot(ReadSnapshotRequest::new(config.clone(), "alpha"))
        .await?
        .snapshot;
    assert_eq!(deleted_alpha, None);
    let snapshot_map: std::collections::BTreeMap<String, Snapshot<TestSnapshot>> =
        read_snapshot_map(fixture.store.snapshot_bucket(), &config).await?;
    assert!(!snapshot_map.contains_key("alpha"));

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_snapshot_checkpoint_concurrent_advances_once_and_never_rewinds() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let config = SnapshotStoreConfig::with_checkpoint_name("snapshots.checkpoint.", "last_event_sequence");

    let first_results = join_all((0..24).map(|_| {
        let bucket = fixture.store.snapshot_bucket().clone();
        let config = config.clone();
        async move { maybe_advance_checkpoint(&bucket, &config, 1).await }
    }))
    .await;
    for result in first_results {
        result?;
    }
    assert_eq!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await?, 1);

    let second_results = join_all((0..24).map(|_| {
        let bucket = fixture.store.snapshot_bucket().clone();
        let config = config.clone();
        async move { maybe_advance_checkpoint(&bucket, &config, 2).await }
    }))
    .await;
    for result in second_results {
        result?;
    }
    assert_eq!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await?, 2);

    maybe_advance_checkpoint(fixture.store.snapshot_bucket(), &config, 4).await?;
    assert_eq!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await?, 2);
    maybe_advance_checkpoint(fixture.store.snapshot_bucket(), &config, 3).await?;
    assert_eq!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await?, 3);
    maybe_advance_checkpoint(fixture.store.snapshot_bucket(), &config, 2).await?;
    assert_eq!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await?, 3);

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_snapshot_checkpoint_concurrent_writes_are_idempotent() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let config = SnapshotStoreConfig::with_checkpoint_name("snapshots.write-checkpoint.", "last_event_sequence");

    let results = join_all((0..24).map(|_| {
        let bucket = fixture.store.snapshot_bucket().clone();
        let config = config.clone();
        async move { write_checkpoint(&bucket, &config, 9).await }
    }))
    .await;
    for result in results {
        result?;
    }
    assert_eq!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await?, 9);

    fixture.store.snapshot_bucket().delete(checkpoint_key(&config)?).await?;
    assert_eq!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await?, 0);
    write_checkpoint(fixture.store.snapshot_bucket(), &config, 10).await?;
    assert_eq!(read_checkpoint(fixture.store.snapshot_bucket(), &config).await?, 10);

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_snapshot_store_rejects_invalid_snapshot_key_from_live_bucket_listing() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let config = SnapshotStoreConfig::without_checkpoint("snapshots.invalid");
    fixture
        .store
        .snapshot_bucket()
        .put(
            config.key_prefix(),
            serde_json::to_vec(&Snapshot::new(
                1,
                TestSnapshot {
                    value: "invalid".to_string(),
                },
            ))?
            .into(),
        )
        .await?;

    let result: Result<std::collections::BTreeMap<String, Snapshot<TestSnapshot>>, SnapshotStoreError> =
        read_snapshot_map(fixture.store.snapshot_bucket(), &config).await;
    assert!(matches!(
        result,
        Err(SnapshotStoreError::InvalidSnapshotKey { key }) if key == config.key_prefix()
    ));

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_snapshot_store_rejects_corrupt_snapshot_from_live_bucket_listing() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let config = SnapshotStoreConfig::without_checkpoint("snapshots.corrupt.");
    fixture
        .store
        .write_snapshot(WriteSnapshotRequest::new(
            config.clone(),
            "alpha",
            Snapshot::new(
                1,
                TestSnapshot {
                    value: "alpha-v1".to_string(),
                },
            ),
        ))
        .await?;
    fixture
        .store
        .snapshot_bucket()
        .put(snapshot_key(&config, "corrupt"), "not-json".into())
        .await?;

    let result: Result<std::collections::BTreeMap<String, Snapshot<TestSnapshot>>, SnapshotStoreError> =
        read_snapshot_map(fixture.store.snapshot_bucket(), &config).await;
    match result {
        Err(SnapshotStoreError::Kv { .. }) => {}
        other => return Err(std::io::Error::other(format!("expected corrupt snapshot error, got {other:?}")).into()),
    }

    fixture.delete().await
}

#[tokio::test]
#[ignore = "requires actual NATS JetStream"]
async fn jetstream_snapshot_store_ignores_corrupt_entries_outside_snapshot_namespace() -> TestResult {
    let fixture = JetStreamFixture::new().await?;
    let config = SnapshotStoreConfig::without_checkpoint("snapshots.clean.");
    fixture
        .store
        .write_snapshot(WriteSnapshotRequest::new(
            config.clone(),
            "alpha",
            Snapshot::new(
                1,
                TestSnapshot {
                    value: "alpha-v1".to_string(),
                },
            ),
        ))
        .await?;
    fixture
        .store
        .snapshot_bucket()
        .put("snapshots.other.corrupt", "not-json".into())
        .await?;

    let snapshot_map: std::collections::BTreeMap<String, Snapshot<TestSnapshot>> =
        read_snapshot_map(fixture.store.snapshot_bucket(), &config).await?;
    assert_eq!(snapshot_map.len(), 1);
    assert_eq!(
        snapshot_map.get("alpha"),
        Some(&Snapshot::new(
            1,
            TestSnapshot {
                value: "alpha-v1".to_string(),
            }
        ))
    );

    fixture.delete().await
}
