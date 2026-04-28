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
pub struct StreamReadResult {
    pub current_version: Option<u64>,
    pub events: Vec<RecordedEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendOutcome {
    pub next_expected_version: u64,
}

pub trait StreamRead<StreamId: ?Sized>: Send + Sync {
    type Error;

    fn read_stream(
        &self,
        stream_id: &StreamId,
        from_sequence: u64,
    ) -> impl std::future::Future<Output = Result<StreamReadResult, Self::Error>> + Send;
}

pub trait StreamAppend<StreamId: ?Sized>: Send + Sync {
    type Error;

    fn append_stream(
        &self,
        stream_id: &StreamId,
        stream_state: StreamState,
        events: NonEmpty<EventData>,
    ) -> impl std::future::Future<Output = Result<AppendOutcome, Self::Error>> + Send;
}
