use std::fmt;

/// The revision of a policy history that was current when it was read.
///
/// Zero means the history did not exist, which is how a session records that it
/// consulted a subject that had never been bound (ADR#0071 Decision 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StreamRevision(u64);

impl StreamRevision {
    /// Wraps a revision number.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The revision of a history that does not exist.
    pub const fn missing() -> Self {
        Self(0)
    }

    /// Returns the revision number.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Returns whether this stands for a history that does not exist.
    pub const fn is_missing(self) -> bool {
        self.0 == 0
    }
}

impl From<u64> for StreamRevision {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for StreamRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
