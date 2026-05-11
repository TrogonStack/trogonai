use super::stream_state::StreamState;
use crate::{EventData, NonEmpty};

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
