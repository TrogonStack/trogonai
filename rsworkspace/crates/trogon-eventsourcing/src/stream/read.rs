use super::position::StreamPosition;
use crate::RecordedEvent;

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
