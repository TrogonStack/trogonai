/// Concurrency guard applied when appending events to a stream.
///
/// Set via [`Decider::WRITE_PRECONDITION`](crate::Decider::WRITE_PRECONDITION). The
/// chosen variant is checked against the stream's current state by the persistence
/// layer; if the precondition is not met, the append is rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePrecondition {
    /// Append regardless of current stream state.
    Any,
    /// Append only if the stream already contains events.
    StreamExists,
    /// Append only if the stream is empty (first writer wins).
    NoStream,
}
