use super::stream_position::StreamPosition;
use crate::{Event, Events};

#[derive(Debug, Clone, PartialEq)]
pub struct AppendStreamRequest<'a, StreamId: ?Sized> {
    pub stream_id: &'a StreamId,
    pub stream_write_precondition: StreamWritePrecondition,
    pub events: Events<Event>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendStreamResponse {
    /// The stream high-watermark after the append completed.
    ///
    /// This is the value to store in projections, snapshots, realtime messages,
    /// and later `StreamWritePrecondition::At` preconditions. It is not a "next expected
    /// version" and it is not safe to perform arithmetic on it.
    pub stream_position: StreamPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamWritePrecondition {
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

impl From<Option<StreamPosition>> for StreamWritePrecondition {
    fn from(current_position: Option<StreamPosition>) -> Self {
        match current_position {
            Some(position) => Self::At(position),
            None => Self::NoStream,
        }
    }
}

pub trait StreamAppend<StreamId: ?Sized>: Send + Sync {
    type Error;

    fn append_stream(
        &self,
        request: AppendStreamRequest<'_, StreamId>,
    ) -> impl std::future::Future<Output = Result<AppendStreamResponse, Self::Error>> + Send;
}
