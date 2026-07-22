//! In-memory storage doubles shared by this crate's own unit tests.
//!
//! Integration tests under `tests/` maintain their own copy of equivalent
//! doubles (they cannot see crate-private items), following the same
//! convention documented on [`crate::test_fixture`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, ReadFrom, ReadSnapshotRequest, ReadSnapshotResponse, ReadStreamRequest,
    ReadStreamResponse, Snapshot, SnapshotRead, SnapshotWrite, StreamAppend, StreamEvent, StreamPosition, StreamRead,
    StreamWritePrecondition, WriteSnapshotRequest, WriteSnapshotResponse,
};

use crate::OpaqueSnapshotPayload;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum InfraError {
    #[error("append rejected by write precondition")]
    PreconditionFailed,
}

#[derive(Default)]
struct EventStoreState {
    events: Vec<StreamEvent>,
    reads_from: Vec<ReadFrom>,
}

/// Records every read so tests can assert on replay behavior, not only on
/// the outcome of an execution.
#[derive(Default)]
pub(crate) struct InMemoryEventStore {
    state: Mutex<EventStoreState>,
}

impl InMemoryEventStore {
    pub(crate) fn reads_from(&self) -> Vec<ReadFrom> {
        self.lock().reads_from.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, EventStoreState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn current_position(state: &EventStoreState, stream_id: &str) -> Option<StreamPosition> {
        state
            .events
            .iter()
            .filter(|event| event.stream_id == stream_id)
            .map(|event| event.stream_position)
            .max()
    }
}

impl StreamRead<str> for InMemoryEventStore {
    type Error = InfraError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        let mut state = self.lock();
        state.reads_from.push(request.from);
        let from_sequence = match request.from {
            ReadFrom::Beginning => 1,
            ReadFrom::Position(position) => position.as_u64(),
        };
        Ok(ReadStreamResponse {
            current_position: Self::current_position(&state, request.stream_id),
            events: state
                .events
                .iter()
                .filter(|event| event.stream_id == request.stream_id)
                .filter(|event| event.stream_position.as_u64() >= from_sequence)
                .cloned()
                .collect(),
        })
    }
}

impl StreamAppend<str> for InMemoryEventStore {
    type Error = InfraError;

    async fn append_stream(&self, request: AppendStreamRequest<'_, str>) -> Result<AppendStreamResponse, Self::Error> {
        let mut state = self.lock();
        let current_position = Self::current_position(&state, request.stream_id);
        match request.stream_write_precondition {
            StreamWritePrecondition::Any => {}
            StreamWritePrecondition::StreamExists if current_position.is_some() => {}
            StreamWritePrecondition::NoStream if current_position.is_none() => {}
            StreamWritePrecondition::At(position) if current_position == Some(position) => {}
            _ => return Err(InfraError::PreconditionFailed),
        }

        let mut next_sequence = current_position.map(StreamPosition::as_u64).unwrap_or(0);
        for event in request.events {
            next_sequence += 1;
            state.events.push(StreamEvent {
                stream_id: request.stream_id.to_string(),
                event,
                stream_position: StreamPosition::try_new(next_sequence).expect("sequence starts at one"),
                recorded_at: chrono::Utc::now(),
            });
        }

        Ok(AppendStreamResponse {
            stream_position: StreamPosition::try_new(next_sequence).expect("append stores at least one event"),
        })
    }
}

/// Snapshot store double keyed by the caller-supplied snapshot id.
#[derive(Clone, Default)]
pub(crate) struct InMemorySnapshotStore {
    snapshots: Arc<Mutex<HashMap<String, Snapshot<OpaqueSnapshotPayload>>>>,
}

impl InMemorySnapshotStore {
    pub(crate) fn get(&self, snapshot_id: &str) -> Option<Snapshot<OpaqueSnapshotPayload>> {
        self.lock().get(snapshot_id).cloned()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Snapshot<OpaqueSnapshotPayload>>> {
        self.snapshots.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl SnapshotRead<OpaqueSnapshotPayload, str> for InMemorySnapshotStore {
    type Error = InfraError;

    async fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, str>,
    ) -> Result<ReadSnapshotResponse<OpaqueSnapshotPayload>, Self::Error> {
        Ok(ReadSnapshotResponse {
            snapshot: self.get(request.snapshot_id),
        })
    }
}

impl SnapshotWrite<OpaqueSnapshotPayload, str> for InMemorySnapshotStore {
    type Error = InfraError;

    async fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, OpaqueSnapshotPayload, str>,
    ) -> Result<WriteSnapshotResponse, Self::Error> {
        self.lock().insert(request.snapshot_id.to_string(), request.snapshot);
        Ok(WriteSnapshotResponse)
    }
}
