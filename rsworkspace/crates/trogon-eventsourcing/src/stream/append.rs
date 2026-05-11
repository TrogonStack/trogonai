use super::{position::StreamPosition, state::StreamState};
use crate::{EventData, NonEmpty};

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
