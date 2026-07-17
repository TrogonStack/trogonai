use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::*;
use crate::snapshot::SnapshotPayloadData;
use crate::{EventId, Headers, InvalidSnapshotTypeName, Snapshot, SnapshotTypeName};

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).expect("test stream position must be non-zero")
}

fn event(id: u128, content: &[u8]) -> Event {
    Event {
        id: EventId::from(Uuid::from_u128(id)),
        r#type: "test.event.v1".to_string(),
        content: content.to_vec(),
        headers: Headers::empty(),
    }
}

fn append(
    store: &InMemoryStore,
    stream_id: &str,
    precondition: StreamWritePrecondition,
    events: Vec<Event>,
) -> Result<AppendStreamResponse, StreamAppendError> {
    futures::executor::block_on(store.append_stream(AppendStreamRequest {
        stream_id,
        stream_write_precondition: precondition,
        events,
    }))
}

fn read(store: &InMemoryStore, stream_id: &str, from: ReadFrom) -> ReadStreamResponse {
    futures::executor::block_on(store.read_stream(ReadStreamRequest { stream_id, from })).expect("read never fails")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TestPayload {
    label: String,
}

impl SnapshotPayloadEncode for TestPayload {
    type Error = serde_json::Error;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(self)
    }
}

impl SnapshotPayloadDecode for TestPayload {
    type Error = serde_json::Error;

    fn decode(payload: SnapshotPayloadData<'_>) -> Result<Self, Self::Error> {
        serde_json::from_slice(payload.payload)
    }
}

impl SnapshotType for TestPayload {
    type Error = InvalidSnapshotTypeName;

    fn snapshot_type() -> Result<SnapshotTypeName, Self::Error> {
        SnapshotTypeName::new("test.memory.v1.Snapshot")
    }
}

fn write_snapshot(
    store: &InMemoryStore,
    snapshot_id: &str,
    snapshot: Snapshot<TestPayload>,
) -> Result<WriteSnapshotResponse, SnapshotEncodeError<serde_json::Error, InvalidSnapshotTypeName>> {
    futures::executor::block_on(store.write_snapshot(WriteSnapshotRequest { snapshot_id, snapshot }))
}

fn read_snapshot(
    store: &InMemoryStore,
    snapshot_id: &str,
) -> Result<ReadSnapshotResponse<TestPayload>, SnapshotDecodeError<serde_json::Error, InvalidSnapshotTypeName>> {
    futures::executor::block_on(store.read_snapshot(ReadSnapshotRequest { snapshot_id }))
}

#[test]
fn read_stream_returns_no_current_position_for_an_unknown_stream() {
    let store = InMemoryStore::new();

    let response = read(&store, "unknown", ReadFrom::Beginning);

    assert_eq!(response.current_position, None);
    assert!(response.events.is_empty());
}

#[test]
fn append_stream_creates_a_fresh_stream_with_no_stream_precondition() {
    let store = InMemoryStore::new();

    let response = append(
        &store,
        "orders/1",
        StreamWritePrecondition::NoStream,
        vec![event(1, b"one"), event(2, b"two")],
    )
    .expect("append to a fresh stream must succeed");

    assert_eq!(response.stream_position, position(2));

    let read = read(&store, "orders/1", ReadFrom::Beginning);
    assert_eq!(read.current_position, Some(position(2)));
    assert_eq!(read.events.len(), 2);
    assert_eq!(read.events[0].stream_position, position(1));
    assert_eq!(read.events[1].stream_position, position(2));
    assert_eq!(read.events[0].stream_id, "orders/1");
}

#[test]
fn append_stream_rejects_no_stream_precondition_when_stream_already_exists() {
    let store = InMemoryStore::new();
    append(
        &store,
        "orders/1",
        StreamWritePrecondition::NoStream,
        vec![event(1, b"one")],
    )
    .expect("first append must succeed");

    let error = append(
        &store,
        "orders/1",
        StreamWritePrecondition::NoStream,
        vec![event(2, b"two")],
    )
    .expect_err("second append with NoStream must be rejected");

    assert_eq!(
        error,
        StreamAppendError::WriteConflict {
            stream_id: "orders/1".to_string(),
            expected: StreamWritePrecondition::NoStream,
            current_position: Some(position(1)),
        }
    );
}

#[test]
fn append_stream_rejects_stream_exists_precondition_when_stream_is_missing() {
    let store = InMemoryStore::new();

    let error = append(
        &store,
        "orders/1",
        StreamWritePrecondition::StreamExists,
        vec![event(1, b"one")],
    )
    .expect_err("StreamExists must be rejected for a stream with no events");

    assert_eq!(
        error,
        StreamAppendError::WriteConflict {
            stream_id: "orders/1".to_string(),
            expected: StreamWritePrecondition::StreamExists,
            current_position: None,
        }
    );
}

#[test]
fn append_stream_accepts_stream_exists_precondition_once_the_stream_has_events() {
    let store = InMemoryStore::new();
    append(
        &store,
        "orders/1",
        StreamWritePrecondition::NoStream,
        vec![event(1, b"one")],
    )
    .expect("first append must succeed");

    let response = append(
        &store,
        "orders/1",
        StreamWritePrecondition::StreamExists,
        vec![event(2, b"two")],
    )
    .expect("StreamExists must succeed once the stream has events");

    assert_eq!(response.stream_position, position(2));
}

#[test]
fn append_stream_rejects_at_precondition_when_position_has_moved() {
    let store = InMemoryStore::new();
    append(
        &store,
        "orders/1",
        StreamWritePrecondition::NoStream,
        vec![event(1, b"one")],
    )
    .expect("first append must succeed");
    append(
        &store,
        "orders/1",
        StreamWritePrecondition::At(position(1)),
        vec![event(2, b"two")],
    )
    .expect("second append at the observed position must succeed");

    let error = append(
        &store,
        "orders/1",
        StreamWritePrecondition::At(position(1)),
        vec![event(3, b"three")],
    )
    .expect_err("stale At precondition must be rejected");

    assert_eq!(
        error,
        StreamAppendError::WriteConflict {
            stream_id: "orders/1".to_string(),
            expected: StreamWritePrecondition::At(position(1)),
            current_position: Some(position(2)),
        }
    );
}

#[test]
fn append_stream_accepts_any_precondition_regardless_of_stream_state() {
    let store = InMemoryStore::new();

    append(&store, "orders/1", StreamWritePrecondition::Any, vec![event(1, b"one")])
        .expect("Any must succeed on a missing stream");
    let response = append(&store, "orders/1", StreamWritePrecondition::Any, vec![event(2, b"two")])
        .expect("Any must succeed on an existing stream");

    assert_eq!(response.stream_position, position(2));
}

#[test]
fn append_stream_rejects_empty_events_against_a_stream_with_no_current_position() {
    let store = InMemoryStore::new();

    let error = append(&store, "orders/1", StreamWritePrecondition::NoStream, Vec::new())
        .expect_err("appending zero events without a current position must be rejected");

    assert_eq!(
        error,
        StreamAppendError::EmptyAppendWithoutPosition {
            stream_id: "orders/1".to_string(),
        }
    );
}

#[test]
fn append_stream_accepts_empty_events_when_a_current_position_already_exists() {
    let store = InMemoryStore::new();
    append(
        &store,
        "orders/1",
        StreamWritePrecondition::NoStream,
        vec![event(1, b"one")],
    )
    .expect("first append must succeed");

    let response = append(&store, "orders/1", StreamWritePrecondition::Any, Vec::new())
        .expect("empty append against an existing position must succeed");

    assert_eq!(response.stream_position, position(1));
}

#[test]
fn read_stream_filters_events_starting_at_the_requested_position() {
    let store = InMemoryStore::new();
    append(
        &store,
        "orders/1",
        StreamWritePrecondition::NoStream,
        vec![event(1, b"one"), event(2, b"two"), event(3, b"three")],
    )
    .expect("append must succeed");

    let response = read(&store, "orders/1", ReadFrom::Position(position(2)));

    assert_eq!(response.events.len(), 2);
    assert_eq!(response.events[0].stream_position, position(2));
    assert_eq!(response.events[1].stream_position, position(3));
}

#[test]
fn cloning_the_store_shares_the_same_underlying_state() {
    let store = InMemoryStore::new();
    let handle = store.clone();

    append(
        &handle,
        "orders/1",
        StreamWritePrecondition::NoStream,
        vec![event(1, b"one")],
    )
    .expect("append through the cloned handle must succeed");

    let response = read(&store, "orders/1", ReadFrom::Beginning);
    assert_eq!(response.current_position, Some(position(1)));
}

#[test]
fn read_snapshot_returns_none_when_no_snapshot_was_written() {
    let store = InMemoryStore::new();

    let response = read_snapshot(&store, "orders/1").expect("read must succeed");

    assert!(response.snapshot.is_none());
}

#[test]
fn snapshot_round_trips_through_write_and_read() {
    let store = InMemoryStore::new();
    let snapshot = Snapshot::new(
        position(3),
        TestPayload {
            label: "checkpoint".to_string(),
        },
    );

    write_snapshot(&store, "orders/1", snapshot.clone()).expect("write must succeed");
    let response = read_snapshot(&store, "orders/1").expect("read must succeed");

    assert_eq!(response.snapshot, Some(snapshot));
}

#[test]
fn snapshot_write_overwrites_the_previous_snapshot_for_the_same_id() {
    let store = InMemoryStore::new();
    write_snapshot(
        &store,
        "orders/1",
        Snapshot::new(
            position(1),
            TestPayload {
                label: "first".to_string(),
            },
        ),
    )
    .expect("first write must succeed");

    let second = Snapshot::new(
        position(2),
        TestPayload {
            label: "second".to_string(),
        },
    );
    write_snapshot(&store, "orders/1", second.clone()).expect("second write must succeed");

    let response = read_snapshot(&store, "orders/1").expect("read must succeed");
    assert_eq!(response.snapshot, Some(second));
}
