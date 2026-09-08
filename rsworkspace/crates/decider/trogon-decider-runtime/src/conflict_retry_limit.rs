use std::num::NonZeroU32;

/// Caps how many times one command execution re-reads and re-decides after
/// another writer beat it to the stream.
///
/// An optimistic-concurrency conflict is not a failure of the command. It says
/// the state the decision was made from is no longer the state the append
/// would land on, and the only thing that can be done about it is to read the
/// stream again and decide again. Without a limit configured, that round is
/// the caller's to run: the conflict surfaces as
/// [`CommandError::Append`](crate::CommandError::Append) and the command is
/// over. With one, the execution runs it in place, up to this many extra
/// attempts, and the caller only sees a conflict that outlived all of them.
///
/// The limit counts *retries*, not attempts: the first attempt always happens,
/// so a limit of one means at most two appends are attempted. Zero is not
/// representable, because "no retries" is the absence of a limit rather than a
/// limit of none.
///
/// Retrying is only sound where the precondition came from what this execution
/// read. A command whose declared
/// [`WritePrecondition`](trogon_decider::WritePrecondition) is anything other
/// than `StreamUnchanged`, or an execution carrying a caller-supplied expected
/// revision, is never retried however this limit is set: in those cases the
/// conflict is the answer the caller asked for, and re-reading past it would
/// substitute the runtime's judgment for theirs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConflictRetryLimit(NonZeroU32);

impl ConflictRetryLimit {
    /// Wraps an already validated non-zero retry limit.
    pub const fn new(value: NonZeroU32) -> Self {
        Self(value)
    }

    /// Returns the limit as a plain integer.
    pub const fn as_u32(self) -> u32 {
        self.0.get()
    }

    /// Returns the limit as a non-zero integer.
    pub const fn as_non_zero(self) -> NonZeroU32 {
        self.0
    }

    /// Creates a retry limit after rejecting zero.
    pub const fn try_new(value: u32) -> Result<Self, ConflictRetryLimitError> {
        match NonZeroU32::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(ConflictRetryLimitError { value }),
        }
    }
}

impl TryFrom<u32> for ConflictRetryLimit {
    type Error = ConflictRetryLimitError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl From<ConflictRetryLimit> for u32 {
    fn from(value: ConflictRetryLimit) -> Self {
        value.as_u32()
    }
}

impl std::fmt::Display for ConflictRetryLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_u32().fmt(f)
    }
}

/// Error returned when constructing an invalid [`ConflictRetryLimit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("conflict retry limit must be greater than zero, got {value}")]
pub struct ConflictRetryLimitError {
    value: u32,
}

impl ConflictRetryLimitError {
    /// Returns the rejected retry limit value.
    pub const fn value(self) -> u32 {
        self.value
    }
}

#[cfg(test)]
mod tests;
