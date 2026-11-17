//! Snapshot storage retains its payload type when replay state uses a local wrapper.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::convert::Infallible;
use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};
use trogon_decider_runtime::{
    AppendStreamRequest, CommandExecution, Decider, Decision, Event, EventData, EventDecode, EventDecodeOutcome,
    EventEncode, EventId, EventIdentity, EventType, Evolve, FrequencySnapshot, Headers, ImmediateSnapshotTaskScheduler,
    InMemoryStore, InvalidSnapshotTypeNameError, ReadSnapshotRequest, ReadSnapshotResponse, ReplayLimit, Snapshot,
    SnapshotPayloadData, SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotRead, SnapshotType, SnapshotTypeName,
    SnapshotWrite, Snapshots, StateSnapshot, StreamAppend, StreamPosition, StreamWritePrecondition, WritePrecondition,
    WriteSnapshotRequest,
};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CounterPayload {
    total: u64,
}

impl SnapshotType for CounterPayload {
    type Error = InvalidSnapshotTypeNameError;

    fn snapshot_type() -> Result<SnapshotTypeName, Self::Error> {
        SnapshotTypeName::new("test.counter.v1.State")
    }
}

impl SnapshotPayloadEncode for CounterPayload {
    type Error = serde_json::Error;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(self)
    }
}

impl SnapshotPayloadDecode for CounterPayload {
    type Error = serde_json::Error;

    fn decode(payload: SnapshotPayloadData<'_>) -> Result<Self, Self::Error> {
        serde_json::from_slice(payload.payload)
    }
}

#[derive(Debug)]
struct CounterState(CounterPayload);

impl Default for CounterState {
    fn default() -> Self {
        Self(CounterPayload { total: 10 })
    }
}

impl From<CounterPayload> for CounterState {
    fn from(payload: CounterPayload) -> Self {
        Self(payload)
    }
}

impl From<CounterState> for CounterPayload {
    fn from(state: CounterState) -> Self {
        state.0
    }
}

impl AsRef<CounterPayload> for CounterState {
    fn as_ref(&self) -> &CounterPayload {
        &self.0
    }
}

impl StateSnapshot for CounterState {
    type Payload = CounterPayload;
}

impl Evolve<Added> for CounterState {
    type Error = Infallible;

    fn evolve(mut self, event: &Added) -> Result<Self, Self::Error> {
        self.0.total += event.amount;
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Added {
    amount: u64,
}

impl EventIdentity for Added {}

impl EventType for Added {
    type Error = Infallible;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        Ok("test.counter.v1.Added")
    }
}

impl EventEncode for Added {
    type Error = serde_json::Error;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(self)
    }
}

impl EventDecode for Added {
    type Error = serde_json::Error;

    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        serde_json::from_slice(event.payload).map(EventDecodeOutcome::Decoded)
    }
}

struct Add {
    amount: u64,
}

impl Decider for Add {
    type StreamId = str;
    type State = CounterState;
    type Event = Added;
    type DecideError = Infallible;

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &Self::StreamId {
        "counter-1"
    }

    fn decide(_state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        Ok(Decision::event(Added { amount: command.amount }))
    }
}

fn stored_event(id: u128, amount: u64) -> Event {
    let event = Added { amount };
    Event {
        id: EventId::new(Uuid::from_u128(id)),
        r#type: event.event_type().unwrap().to_string(),
        content: event.encode().unwrap(),
        headers: Headers::empty(),
    }
}

#[tokio::test]
async fn restores_payload_and_replays_suffix_before_persisting_the_next_payload() {
    let store = InMemoryStore::new();
    store
        .append_stream(AppendStreamRequest {
            stream_id: "counter-1",
            stream_write_precondition: StreamWritePrecondition::NoStream,
            events: vec![stored_event(1, 20), stored_event(2, 3)],
        })
        .await
        .unwrap();
    store
        .write_snapshot(WriteSnapshotRequest {
            snapshot_id: "counter-1",
            snapshot: Snapshot::new(StreamPosition::try_new(1).unwrap(), CounterPayload { total: 30 }),
        })
        .await
        .unwrap();

    for (amount, expected_total, expected_position) in [(5, 38, 3), (7, 45, 4)] {
        let command = Add { amount };
        let result = CommandExecution::new(&store, &command)
            .with_snapshot(Snapshots::new(&store, FrequencySnapshot::new(NonZeroU64::MIN)))
            .with_task_runtime(ImmediateSnapshotTaskScheduler)
            .with_replay_limit(ReplayLimit::new(NonZeroU64::MIN))
            .execute()
            .await
            .unwrap();

        assert_eq!(result.state.as_ref().total, expected_total);
        assert_eq!(result.stream_position.as_u64(), expected_position);
        let response: ReadSnapshotResponse<CounterPayload> = store
            .read_snapshot(ReadSnapshotRequest {
                snapshot_id: "counter-1",
            })
            .await
            .unwrap();
        let snapshot = response.snapshot.unwrap();
        assert_eq!(snapshot.position, result.stream_position);
        assert_eq!(&snapshot.payload, result.state.as_ref());

        let returned_payload: CounterPayload = result.state.into();
        assert_eq!(returned_payload.total, expected_total);
    }
}
