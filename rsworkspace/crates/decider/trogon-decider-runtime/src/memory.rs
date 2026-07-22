//! In-memory store test double for decider stream and snapshot contracts.
//!
//! [`InMemoryStore`] implements [`StreamRead`], [`StreamAppend`],
//! [`SnapshotRead`], and [`SnapshotWrite`] entirely in process memory. It
//! exists so tests can exercise a [`CommandExecution`](crate::CommandExecution)
//! or a projection against real optimistic concurrency semantics without
//! standing up a backing store such as NATS JetStream or Postgres.
//!
//! A stream's current position is the number of events appended to it, so
//! `StreamWritePrecondition::Any`, `StreamWritePrecondition::NoStream`,
//! `StreamWritePrecondition::StreamExists`, and `StreamWritePrecondition::At`
//! are all enforced against that count before an append is accepted, the same
//! way a storage-backed adapter enforces them against a subject sequence or a
//! row version.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;

use crate::snapshot::{
    EncodedSnapshot, ReadSnapshotRequest, ReadSnapshotResponse, SnapshotDecodeError, SnapshotEncodeError,
    SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotRead, SnapshotType, SnapshotWrite, WriteSnapshotRequest,
    WriteSnapshotResponse, decode_snapshot, encode_snapshot,
};
use crate::stream::{
    AppendStreamRequest, AppendStreamResponse, ReadFrom, ReadStreamRequest, ReadStreamResponse, StreamAppend,
    StreamPosition, StreamRead, StreamWritePrecondition,
};
use crate::{Event, StreamEvent};

/// In-memory implementation of [`StreamRead`], [`StreamAppend`],
/// [`SnapshotRead`], and [`SnapshotWrite`] for tests.
///
/// Cloning an [`InMemoryStore`] returns a handle to the same underlying
/// state; create one store per test fixture with [`InMemoryStore::new`] and
/// clone the handle to share it across concurrent readers and writers.
#[derive(Clone, Default)]
pub struct InMemoryStore {
    streams: Arc<Mutex<HashMap<String, Vec<Event>>>>,
    snapshots: Arc<Mutex<HashMap<String, EncodedSnapshot>>>,
}

impl InMemoryStore {
    /// Creates an empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Error returned when an append's [`StreamWritePrecondition`] is not
/// satisfied by an [`InMemoryStore`] stream's current state.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StreamAppendError {
    /// The stream's current position did not satisfy the write precondition.
    #[error(
        "stream '{stream_id}' write precondition violated: expected {expected:?}, {}",
        describe_current_position(current_position)
    )]
    WriteConflict {
        /// Stream identity that failed its write precondition.
        stream_id: String,
        /// Precondition supplied by the caller.
        expected: StreamWritePrecondition,
        /// Stream position observed immediately before the append was attempted.
        current_position: Option<StreamPosition>,
    },
    /// Zero events were appended to a stream that has no current position, so
    /// no [`StreamPosition`] exists to report as the result of the append.
    #[error("cannot append zero events to stream '{stream_id}' with no current position")]
    EmptyAppendWithoutPosition {
        /// Stream identity that the empty append targeted.
        stream_id: String,
    },
}

fn describe_current_position(current_position: &Option<StreamPosition>) -> String {
    match current_position {
        Some(position) => format!("current position is {position}"),
        None => "stream has no current position".to_string(),
    }
}

fn check_write_precondition(
    stream_id: &str,
    precondition: StreamWritePrecondition,
    current_position: Option<StreamPosition>,
) -> Result<(), StreamAppendError> {
    let satisfied = match precondition {
        StreamWritePrecondition::Any => true,
        StreamWritePrecondition::StreamExists => current_position.is_some(),
        StreamWritePrecondition::NoStream => current_position.is_none(),
        StreamWritePrecondition::At(expected) => current_position == Some(expected),
    };

    if satisfied {
        Ok(())
    } else {
        Err(StreamAppendError::WriteConflict {
            stream_id: stream_id.to_string(),
            expected: precondition,
            current_position,
        })
    }
}

impl<StreamId> StreamRead<StreamId> for InMemoryStore
where
    StreamId: AsRef<str> + ToString + Send + Sync + ?Sized,
{
    type Error = std::convert::Infallible;

    async fn read_stream(&self, request: ReadStreamRequest<'_, StreamId>) -> Result<ReadStreamResponse, Self::Error> {
        let stream_id = request.stream_id;
        let from_sequence = match request.from {
            ReadFrom::Beginning => 1,
            ReadFrom::Position(position) => position.as_u64(),
        };

        let events = self
            .streams
            .lock()
            .expect("in-memory event store mutex poisoned")
            .get(stream_id.as_ref())
            .cloned()
            .unwrap_or_default();

        let current_position = StreamPosition::try_new(events.len() as u64).ok();
        let recorded_at = Utc::now();
        let mut matched = Vec::new();
        for (index, event) in events.into_iter().enumerate() {
            let sequence = index as u64 + 1;
            if sequence < from_sequence {
                continue;
            }
            matched.push(StreamEvent {
                stream_id: stream_id.to_string(),
                event,
                stream_position: StreamPosition::try_new(sequence).expect("sequence is always non-zero"),
                recorded_at,
            });
        }

        Ok(ReadStreamResponse {
            current_position,
            events: matched,
        })
    }
}

impl<StreamId> StreamAppend<StreamId> for InMemoryStore
where
    StreamId: AsRef<str> + ToString + Send + Sync + ?Sized,
{
    type Error = StreamAppendError;

    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, StreamId>,
    ) -> Result<AppendStreamResponse, Self::Error> {
        let stream_id = request.stream_id.as_ref();
        let mut streams = self.streams.lock().expect("in-memory event store mutex poisoned");
        let stored = streams.entry(stream_id.to_string()).or_default();
        let current_position = StreamPosition::try_new(stored.len() as u64).ok();

        check_write_precondition(stream_id, request.stream_write_precondition, current_position)?;

        if request.events.is_empty() && current_position.is_none() {
            return Err(StreamAppendError::EmptyAppendWithoutPosition {
                stream_id: stream_id.to_string(),
            });
        }

        stored.extend(request.events);
        let stream_position = StreamPosition::try_new(stored.len() as u64)
            .expect("stream already had events or new events were appended");

        Ok(AppendStreamResponse { stream_position })
    }
}

impl<Payload, SnapshotId> SnapshotRead<Payload, SnapshotId> for InMemoryStore
where
    SnapshotId: AsRef<str> + Send + Sync + ?Sized,
    Payload: SnapshotPayloadDecode + SnapshotType + Send,
    <Payload as SnapshotPayloadDecode>::Error: std::error::Error + Send + Sync + 'static,
    <Payload as SnapshotType>::Error: std::error::Error + Send + Sync + 'static,
{
    type Error = SnapshotDecodeError<<Payload as SnapshotPayloadDecode>::Error, <Payload as SnapshotType>::Error>;

    async fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, SnapshotId>,
    ) -> Result<ReadSnapshotResponse<Payload>, Self::Error> {
        let stored = self
            .snapshots
            .lock()
            .expect("in-memory snapshot store mutex poisoned")
            .get(request.snapshot_id.as_ref())
            .cloned();

        let Some(encoded) = stored else {
            return Ok(ReadSnapshotResponse { snapshot: None });
        };

        let snapshot = decode_snapshot::<Payload>(encoded)?;
        Ok(ReadSnapshotResponse {
            snapshot: Some(snapshot),
        })
    }
}

impl<Payload, SnapshotId> SnapshotWrite<Payload, SnapshotId> for InMemoryStore
where
    SnapshotId: AsRef<str> + Send + Sync + ?Sized,
    Payload: SnapshotPayloadEncode + SnapshotType + Send,
    <Payload as SnapshotPayloadEncode>::Error: std::error::Error + Send + Sync + 'static,
    <Payload as SnapshotType>::Error: std::error::Error + Send + Sync + 'static,
{
    type Error = SnapshotEncodeError<<Payload as SnapshotPayloadEncode>::Error, <Payload as SnapshotType>::Error>;

    async fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, Payload, SnapshotId>,
    ) -> Result<WriteSnapshotResponse, Self::Error> {
        let encoded = encode_snapshot(&request.snapshot)?;
        self.snapshots
            .lock()
            .expect("in-memory snapshot store mutex poisoned")
            .insert(request.snapshot_id.as_ref().to_string(), encoded);
        Ok(WriteSnapshotResponse)
    }
}

#[cfg(test)]
mod tests;
