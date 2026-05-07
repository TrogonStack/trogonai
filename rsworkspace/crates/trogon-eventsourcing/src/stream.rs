use crate::{EventData, NonEmpty, RecordedEvent};

mod position;

pub use position::{InvalidStreamPosition, StreamPosition};

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
