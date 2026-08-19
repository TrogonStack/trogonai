use super::stream_position::StreamPosition;
use crate::Event;

/// Request to append events to one stream.
#[derive(Debug, Clone, PartialEq)]
pub struct AppendStreamRequest<'a, StreamId: ?Sized> {
    /// Stream identity in the caller's domain-specific representation.
    pub stream_id: &'a StreamId,
    /// Optimistic concurrency condition for the append.
    pub stream_write_precondition: StreamWritePrecondition,
    /// Event envelopes to append atomically in order.
    pub events: Vec<Event>,
}

/// Result of a successful append.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendStreamResponse {
    /// The stream high-watermark after the append completed.
    ///
    /// This is the value to store in projections, snapshots, realtime messages,
    /// and later `StreamWritePrecondition::At` preconditions. It is not a "next expected
    /// version" and it is not safe to perform arithmetic on it.
    pub stream_position: StreamPosition,
}

/// Optimistic concurrency precondition for appending events.
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

/// What an append failure says about whether attempting it again could succeed.
///
/// Only the store knows which of its errors mean "the stream moved". Naming
/// that here keeps the distinction typed rather than inferred from an error
/// message, and it is the whole input the optimistic-concurrency retry loop
/// needs: contention is worth another read-decide-append round, and nothing
/// else is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendFailure {
    /// The stream did not satisfy the write precondition, because another
    /// writer appended between this execution's read and its append.
    WriteConflict,
    /// Anything else. The same append attempted again fails the same way.
    Fatal,
}

/// Appends event envelopes to a stream.
///
/// Implementations should preserve the caller's precondition semantics while
/// translating them to the backend's native concurrency primitive.
pub trait StreamAppend<StreamId: ?Sized>: Send + Sync {
    /// Backend-specific append error.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Appends the request events and returns the resulting stream position.
    fn append_stream(
        &self,
        request: AppendStreamRequest<'_, StreamId>,
    ) -> impl std::future::Future<Output = Result<AppendStreamResponse, Self::Error>> + Send;

    /// Says whether `error` was the stream moving under the append.
    ///
    /// Defaults to [`AppendFailure::Fatal`], so a store that has not
    /// classified its errors never has a command retried behind its back.
    /// Overriding this is what opts a store into
    /// [`CommandExecution::with_conflict_retry`](crate::CommandExecution::with_conflict_retry);
    /// a store that leaves it alone keeps returning conflicts to its caller
    /// exactly as before.
    fn classify_append_failure(&self, error: &Self::Error) -> AppendFailure {
        let _ = error;
        AppendFailure::Fatal
    }
}
