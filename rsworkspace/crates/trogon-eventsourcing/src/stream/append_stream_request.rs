use super::stream_write_precondition::StreamWritePrecondition;
use crate::{Event, Events};

#[derive(Debug, Clone, PartialEq)]
pub struct AppendStreamRequest<'a, StreamId: ?Sized> {
    pub stream_id: &'a StreamId,
    pub stream_write_precondition: StreamWritePrecondition,
    pub events: Events<Event>,
}

impl<'a, StreamId: ?Sized> AppendStreamRequest<'a, StreamId> {
    pub const fn new(
        stream_id: &'a StreamId,
        stream_write_precondition: StreamWritePrecondition,
        events: Events<Event>,
    ) -> Self {
        Self {
            stream_id,
            stream_write_precondition,
            events,
        }
    }
}
