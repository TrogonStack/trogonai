use super::stream_write_precondition::StreamWritePrecondition;
use crate::{Event, Events};

#[derive(Debug, Clone, PartialEq)]
pub struct AppendStreamRequest<'a, StreamId: ?Sized> {
    pub stream_id: &'a StreamId,
    pub stream_write_precondition: StreamWritePrecondition,
    pub events: Events<Event>,
}
