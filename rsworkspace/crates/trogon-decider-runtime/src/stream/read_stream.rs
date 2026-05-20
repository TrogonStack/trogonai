use super::stream_position::StreamPosition;
use crate::StreamEvent;

/// Where a stream read begins.
///
/// Inclusive semantics match KurrentDB's `StreamPosition::Position` and
/// JetStream's `OptStartSeq`: `Position(p)` returns events whose position is
/// greater than or equal to `p`. Adapters translate to their backend's native
/// "start at" primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadFrom {
    Beginning,
    Position(StreamPosition),
}

impl ReadFrom {
    /// Reads strictly after the given position.
    ///
    /// Encapsulates the `+1` arithmetic that checkpoint-resume requires when a
    /// checkpoint records the position of its last applied event. The result
    /// remains inclusive `Position(p + 1)`, but callers see intent, not math.
    pub fn after(position: StreamPosition) -> Result<Self, ReadAfterOverflow> {
        let next = position
            .as_non_zero()
            .checked_add(1)
            .ok_or(ReadAfterOverflow { position })?;
        Ok(Self::Position(StreamPosition::new(next)))
    }
}

/// Error returned when `ReadFrom::after` would overflow `u64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadAfterOverflow {
    /// Position that could not be advanced.
    pub position: StreamPosition,
}

impl std::fmt::Display for ReadAfterOverflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cannot read after position {}: u64 overflow", self.position)
    }
}

impl std::error::Error for ReadAfterOverflow {}

/// Request to read events from one stream.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadStreamRequest<'a, StreamId: ?Sized> {
    /// Stream identity in the caller's domain-specific representation.
    pub stream_id: &'a StreamId,
    /// Starting point for the stream read.
    pub from: ReadFrom,
}

/// Result of reading one stream.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadStreamResponse {
    /// The latest comparable stream high-watermark observed by the store.
    ///
    /// This value is `None` when the stream has no current position. When it is
    /// present, callers may compare it with another `StreamPosition` from the
    /// same stream to answer freshness questions. Callers must not treat it as
    /// a gapless revision or event count.
    pub current_position: Option<StreamPosition>,
    /// Events returned by the read in stream order.
    pub events: Vec<StreamEvent>,
}

/// Reads event envelopes from a stream.
///
/// Implementations should return events in stream order and preserve the
/// inclusive semantics of [`ReadFrom::Position`].
pub trait StreamRead<StreamId: ?Sized>: Send + Sync {
    /// Backend-specific read error.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Reads events from the requested stream.
    fn read_stream(
        &self,
        request: ReadStreamRequest<'_, StreamId>,
    ) -> impl std::future::Future<Output = Result<ReadStreamResponse, Self::Error>> + Send;
}
