//! In-memory storage doubles for exercising WASM command execution end to end.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, ReadFrom, ReadSnapshotRequest, ReadSnapshotResponse, ReadStreamRequest,
    ReadStreamResponse, Snapshot, SnapshotRead, SnapshotWrite, StreamAppend, StreamEvent, StreamPosition, StreamRead,
    StreamWritePrecondition, WriteSnapshotRequest, WriteSnapshotResponse,
};
use trogon_decider_wasm_runtime::OpaqueSnapshotPayload;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InfraError {
    #[error("append rejected by write precondition")]
    PreconditionRejected,
    #[error("snapshot write failed")]
    SnapshotWriteFailed,
    #[error("snapshot read failed")]
    SnapshotReadFailed,
}

#[derive(Default)]
struct EventStoreState {
    events: Vec<StreamEvent>,
    reads_from: Vec<ReadFrom>,
    write_preconditions: Vec<StreamWritePrecondition>,
}

/// Records every read and append so tests can assert on the execution's
/// storage interaction, not only its outcome.
#[derive(Default)]
pub struct InMemoryEventStore {
    state: Mutex<EventStoreState>,
}

impl InMemoryEventStore {
    pub fn read_stream_calls(&self) -> usize {
        self.lock().reads_from.len()
    }

    pub fn reads_from(&self) -> Vec<ReadFrom> {
        self.lock().reads_from.clone()
    }

    pub fn write_preconditions(&self) -> Vec<StreamWritePrecondition> {
        self.lock().write_preconditions.clone()
    }

    pub fn stored_event_types(&self, stream_id: &str) -> Vec<String> {
        self.lock()
            .events
            .iter()
            .filter(|event| event.stream_id == stream_id)
            .map(|event| event.event.r#type.clone())
            .collect()
    }

    pub fn stored_events(&self, stream_id: &str) -> Vec<trogon_decider_runtime::Event> {
        self.lock()
            .events
            .iter()
            .filter(|event| event.stream_id == stream_id)
            .map(|event| event.event.clone())
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, EventStoreState> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
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
        state.write_preconditions.push(request.stream_write_precondition);
        let current_position = Self::current_position(&state, request.stream_id);
        match request.stream_write_precondition {
            StreamWritePrecondition::Any => {}
            StreamWritePrecondition::StreamExists if current_position.is_some() => {}
            StreamWritePrecondition::NoStream if current_position.is_none() => {}
            StreamWritePrecondition::At(position) if current_position == Some(position) => {}
            _ => return Err(InfraError::PreconditionRejected),
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

/// Shared snapshot store double keyed by the caller-supplied snapshot id.
#[derive(Clone, Default)]
pub struct InMemorySnapshotStore {
    snapshots: Arc<Mutex<HashMap<String, Snapshot<OpaqueSnapshotPayload>>>>,
    fail_writes: Arc<std::sync::atomic::AtomicBool>,
    fail_reads: Arc<std::sync::atomic::AtomicBool>,
}

impl InMemorySnapshotStore {
    pub fn insert(&self, snapshot_id: &str, snapshot: Snapshot<OpaqueSnapshotPayload>) {
        self.lock().insert(snapshot_id.to_string(), snapshot);
    }

    pub fn fail_writes(&self) {
        self.fail_writes.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn fail_reads(&self) {
        self.fail_reads.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn get(&self, snapshot_id: &str) -> Option<Snapshot<OpaqueSnapshotPayload>> {
        self.lock().get(snapshot_id).cloned()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Snapshot<OpaqueSnapshotPayload>>> {
        self.snapshots.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl SnapshotRead<OpaqueSnapshotPayload, str> for InMemorySnapshotStore {
    type Error = InfraError;

    async fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, str>,
    ) -> Result<ReadSnapshotResponse<OpaqueSnapshotPayload>, Self::Error> {
        if self.fail_reads.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(InfraError::SnapshotReadFailed);
        }
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
        if self.fail_writes.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(InfraError::SnapshotWriteFailed);
        }
        self.insert(request.snapshot_id, request.snapshot);
        Ok(WriteSnapshotResponse)
    }
}
