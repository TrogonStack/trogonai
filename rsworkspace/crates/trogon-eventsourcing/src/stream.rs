use crate::{Decide, EventData, NonEmpty, RecordedEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    Any,
    StreamExists,
    NoStream,
    StreamRevision(u64),
}

impl StreamState {
    pub const fn from_current_version(current_version: Option<u64>) -> Self {
        match current_version {
            Some(version) => Self::StreamRevision(version),
            None => Self::NoStream,
        }
    }
}

pub fn resolve_stream_state<C>(write_precondition: Option<StreamState>, current_version: Option<u64>) -> StreamState
where
    C: Decide,
{
    C::REQUIRED_WRITE_PRECONDITION
        .or(write_precondition)
        .unwrap_or_else(|| StreamState::from_current_version(current_version))
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
    pub current_version: Option<u64>,
    pub events: Vec<RecordedEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendStreamResponse {
    pub next_expected_version: u64,
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
