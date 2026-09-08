use std::num::NonZeroU64;

/// Caps how many stream events one command execution holds in memory at a time
/// while replaying history.
///
/// Without a chunk size, replay is one read: the whole history after the
/// snapshot arrives as a single `Vec<StreamEvent>` and is folded from there, so
/// peak memory is proportional to the stream rather than to anything the host
/// chose. With one, the same history is walked in reads of at most this many
/// events, each folded and dropped before the next is fetched, so peak memory
/// is proportional to the chunk instead.
///
/// This is a memory bound, not a correctness bound. A stream is pinned to the
/// position the first read observed, so the chunks a command folds are exactly
/// the history the append is then guarded against, however far the stream moves
/// while the chunks are being read.
///
/// Like [`ReplayLimit`](crate::ReplayLimit), what it actually bounds depends on
/// the store: a [`StreamRead`](crate::StreamRead) implementation still on the
/// default `read_stream_bounded` fetches the whole stream on the first read, and
/// chunking it afterwards would bound nothing it had not already paid for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReplayChunkSize(NonZeroU64);

impl ReplayChunkSize {
    /// Wraps an already validated non-zero chunk size.
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    /// Returns the chunk size as a plain integer.
    pub const fn as_u64(self) -> u64 {
        self.0.get()
    }

    /// Returns the chunk size as a non-zero integer.
    pub const fn as_non_zero(self) -> NonZeroU64 {
        self.0
    }

    /// Creates a chunk size after rejecting zero.
    pub const fn try_new(value: u64) -> Result<Self, ReplayChunkSizeError> {
        match NonZeroU64::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(ReplayChunkSizeError { value }),
        }
    }
}

impl TryFrom<u64> for ReplayChunkSize {
    type Error = ReplayChunkSizeError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl From<ReplayChunkSize> for u64 {
    fn from(value: ReplayChunkSize) -> Self {
        value.as_u64()
    }
}

impl std::fmt::Display for ReplayChunkSize {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Error returned when constructing an invalid [`ReplayChunkSize`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("replay chunk size must be greater than zero, got {value}")]
pub struct ReplayChunkSizeError {
    value: u64,
}

impl ReplayChunkSizeError {
    /// Returns the rejected value.
    pub const fn value(self) -> u64 {
        self.value
    }
}

#[cfg(test)]
mod tests;
