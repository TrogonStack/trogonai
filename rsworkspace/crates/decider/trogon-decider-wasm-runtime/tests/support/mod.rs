//! In-memory storage doubles for exercising WASM command execution end to end.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use trogon_decider_runtime::{
    AppendFailure, AppendStreamRequest, AppendStreamResponse, ReadFrom, ReadSnapshotRequest, ReadSnapshotResponse,
    ReadStreamRequest, ReadStreamResponse, Snapshot, SnapshotRead, SnapshotWrite, StreamAppend, StreamEvent,
    StreamPosition, StreamRead, StreamWritePrecondition, WriteSnapshotRequest, WriteSnapshotResponse,
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
    read_bounds: Vec<Option<u64>>,
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

    /// `max_events` requested by each read, in order, with `None` for reads
    /// that asked for the whole stream.
    pub fn read_bounds(&self) -> Vec<Option<u64>> {
        self.lock().read_bounds.clone()
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

    fn read(&self, request: ReadStreamRequest<'_, str>, max_events: Option<u64>) -> ReadStreamResponse {
        let mut state = self.lock();
        state.reads_from.push(request.from);
        state.read_bounds.push(max_events);
        let from_sequence = match request.from {
            ReadFrom::Beginning => 1,
            ReadFrom::Position(position) => position.as_u64(),
        };
        let current_position = Self::current_position(&state, request.stream_id);
        let mut events: Vec<StreamEvent> = state
            .events
            .iter()
            .filter(|event| event.stream_id == request.stream_id)
            .filter(|event| event.stream_position.as_u64() >= from_sequence)
            .cloned()
            .collect();
        if let Some(max_events) = max_events {
            events.truncate(usize::try_from(max_events).unwrap_or(usize::MAX));
        }

        ReadStreamResponse {
            current_position,
            events,
        }
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
        Ok(self.read(request, None))
    }

    async fn read_stream_bounded(
        &self,
        request: ReadStreamRequest<'_, str>,
        max_events: u64,
    ) -> Result<ReadStreamResponse, Self::Error> {
        Ok(self.read(request, Some(max_events)))
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
    read_snapshot_calls: Arc<std::sync::atomic::AtomicUsize>,
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

    pub fn read_snapshot_calls(&self) -> usize {
        self.read_snapshot_calls.load(std::sync::atomic::Ordering::SeqCst)
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
        self.read_snapshot_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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

/// An event store with another writer racing on the same stream.
///
/// Rejects the first `conflicts` appends and classifies them as contention, so
/// an execution configured to retry re-reads and decides again. Everything
/// else delegates to the [`InMemoryEventStore`] underneath.
#[derive(Default)]
pub struct ContendedEventStore {
    inner: InMemoryEventStore,
    conflicts_left: Mutex<u32>,
    append_attempts: Mutex<Vec<StreamWritePrecondition>>,
}

impl ContendedEventStore {
    /// Makes the next `conflicts` appends lose the race.
    pub fn contend(&self, conflicts: u32) {
        *self.lock_conflicts() = conflicts;
    }

    pub fn read_stream_calls(&self) -> usize {
        self.inner.read_stream_calls()
    }

    /// The precondition of every append attempted, rejected ones included.
    pub fn append_attempts(&self) -> Vec<StreamWritePrecondition> {
        self.append_attempts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn stored_event_types(&self, stream_id: &str) -> Vec<String> {
        self.inner.stored_event_types(stream_id)
    }

    fn lock_conflicts(&self) -> std::sync::MutexGuard<'_, u32> {
        self.conflicts_left
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl StreamRead<str> for ContendedEventStore {
    type Error = InfraError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        self.inner.read_stream(request).await
    }

    async fn read_stream_bounded(
        &self,
        request: ReadStreamRequest<'_, str>,
        max_events: u64,
    ) -> Result<ReadStreamResponse, Self::Error> {
        self.inner.read_stream_bounded(request, max_events).await
    }
}

impl StreamAppend<str> for ContendedEventStore {
    type Error = InfraError;

    async fn append_stream(&self, request: AppendStreamRequest<'_, str>) -> Result<AppendStreamResponse, Self::Error> {
        self.append_attempts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request.stream_write_precondition);

        let contended = {
            let mut conflicts_left = self.lock_conflicts();
            let contended = *conflicts_left > 0;
            *conflicts_left = conflicts_left.saturating_sub(1);
            contended
        };
        if contended {
            return Err(InfraError::PreconditionRejected);
        }

        self.inner.append_stream(request).await
    }

    fn classify_append_failure(&self, error: &Self::Error) -> AppendFailure {
        match error {
            InfraError::PreconditionRejected => AppendFailure::WriteConflict,
            InfraError::SnapshotWriteFailed | InfraError::SnapshotReadFailed => AppendFailure::Fatal,
        }
    }
}
