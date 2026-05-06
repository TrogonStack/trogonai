use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

use crate::{EventData, NonEmpty, RecordedEvent};

/// A comparable high-watermark for one event stream.
///
/// `StreamPosition` is deliberately **not** a stream revision, event count, or
/// gapless version. It is a monotonic position that can answer questions like
/// "has this stream advanced past the position I observed?".
///
/// Valid use cases:
/// - optimistic concurrency with `StreamState::At`
/// - projection freshness checks
/// - snapshot checkpoints
/// - dropping stale realtime updates
///
/// Invalid assumptions:
/// - positions are gapless
/// - positions identify the Nth event in a stream
/// - callers can safely add `1`, subtract, or apply modulo arithmetic
///
/// Storage adapters decide how to map this value. EventStoreDB can map it to a
/// stream revision. JetStream can map it to the last subject sequence. Both are
/// valid because this type promises ordering, not revision semantics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct StreamPosition(NonZeroU64);

impl StreamPosition {
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }

    pub const fn try_new(value: u64) -> Result<Self, InvalidStreamPosition> {
        match NonZeroU64::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(InvalidStreamPosition { value }),
        }
    }
}

impl TryFrom<u64> for StreamPosition {
    type Error = InvalidStreamPosition;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl From<StreamPosition> for u64 {
    fn from(value: StreamPosition) -> Self {
        value.get()
    }
}

impl std::fmt::Display for StreamPosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.get().fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidStreamPosition {
    value: u64,
}

impl InvalidStreamPosition {
    pub const fn value(self) -> u64 {
        self.value
    }
}

impl std::fmt::Display for InvalidStreamPosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stream position must be greater than zero, got {}", self.value)
    }
}

impl std::error::Error for InvalidStreamPosition {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    /// Append without checking the stream's current position.
    Any,
    /// Append only if the stream already has at least one event.
    StreamExists,
    /// Append only if the stream has no events.
    NoStream,
    /// Append only if the stream is still at the observed position.
    ///
    /// This is an OCC precondition over `StreamPosition`, not a revision check.
    /// For EventStoreDB this may map to an expected stream revision. For
    /// JetStream this may map to the expected last subject sequence.
    At(StreamPosition),
}

impl StreamState {
    pub const fn from_current_position(current_position: Option<StreamPosition>) -> Self {
        match current_position {
            Some(position) => Self::At(position),
            None => Self::NoStream,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadStreamRequest<'a, StreamId: ?Sized> {
    pub stream_id: &'a StreamId,
    pub from_sequence: u64,
}

impl<'a, StreamId: ?Sized> ReadStreamRequest<'a, StreamId> {
    pub const fn new(stream_id: &'a StreamId, from_sequence: u64) -> Self {
        Self {
            stream_id,
            from_sequence,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadStreamResponse {
    /// The latest comparable stream high-watermark observed by the store.
    ///
    /// This value is `None` when the stream has no current position. When it is
    /// present, callers may compare it with another `StreamPosition` from the
    /// same stream to answer freshness questions. Callers must not treat it as
    /// a gapless revision or event count.
    pub current_position: Option<StreamPosition>,
    pub events: Vec<RecordedEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendStreamResponse {
    /// The stream high-watermark after the append completed.
    ///
    /// This is the value to store in projections, snapshots, realtime messages,
    /// and later `StreamState::At` preconditions. It is not a "next expected
    /// version" and it is not safe to perform arithmetic on it.
    pub stream_position: StreamPosition,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppendStreamRequest<'a, StreamId: ?Sized> {
    pub stream_id: &'a StreamId,
    pub stream_state: StreamState,
    pub events: NonEmpty<EventData>,
}

impl<'a, StreamId: ?Sized> AppendStreamRequest<'a, StreamId> {
    pub const fn new(stream_id: &'a StreamId, stream_state: StreamState, events: NonEmpty<EventData>) -> Self {
        Self {
            stream_id,
            stream_state,
            events,
        }
    }
}

pub trait StreamRead<StreamId: ?Sized>: Send + Sync {
    type Error;

    fn read_stream(
        &self,
        request: ReadStreamRequest<'_, StreamId>,
    ) -> impl std::future::Future<Output = Result<ReadStreamResponse, Self::Error>> + Send;
}

pub trait StreamAppend<StreamId: ?Sized>: Send + Sync {
    type Error;

    fn append_stream(
        &self,
        request: AppendStreamRequest<'_, StreamId>,
    ) -> impl std::future::Future<Output = Result<AppendStreamResponse, Self::Error>> + Send;
}
