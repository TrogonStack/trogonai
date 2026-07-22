use std::num::NonZeroU64;

/// Caps how many stream events one command execution may replay before
/// folding them into state.
///
/// Without a limit, a command execution reads and folds every event a stream
/// has ever recorded when no snapshot exists, or every event recorded after
/// the loaded snapshot's position. A misconfigured or forgotten snapshot
/// policy then grows per-command latency without bound and goes unnoticed.
/// Setting a [`ReplayLimit`] fails the command with a typed error once a
/// stream read returns more events than the limit, before any folding
/// happens. The check runs after the read, so it makes the misconfiguration
/// fail loudly rather than bounding the read itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReplayLimit(NonZeroU64);

impl ReplayLimit {
    /// Wraps an already validated non-zero replay limit.
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    /// Returns the limit as a plain integer.
    pub const fn as_u64(self) -> u64 {
        self.0.get()
    }

    /// Returns the limit as a non-zero integer.
    pub const fn as_non_zero(self) -> NonZeroU64 {
        self.0
    }

    /// Creates a replay limit after rejecting zero.
    pub const fn try_new(value: u64) -> Result<Self, ReplayLimitError> {
        match NonZeroU64::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(ReplayLimitError { value }),
        }
    }
}

impl TryFrom<u64> for ReplayLimit {
    type Error = ReplayLimitError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl From<ReplayLimit> for u64 {
    fn from(value: ReplayLimit) -> Self {
        value.as_u64()
    }
}

impl std::fmt::Display for ReplayLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_u64().fmt(f)
    }
}

/// Error returned when constructing an invalid [`ReplayLimit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("replay limit must be greater than zero, got {value}")]
pub struct ReplayLimitError {
    value: u64,
}

impl ReplayLimitError {
    /// Returns the rejected replay limit value.
    pub const fn value(self) -> u64 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_and_reports_a_non_zero_value() {
        let non_zero = NonZeroU64::new(42).unwrap();
        let limit = ReplayLimit::new(non_zero);

        assert_eq!(limit.as_u64(), 42);
        assert_eq!(limit.as_non_zero(), non_zero);
        assert_eq!(ReplayLimit::try_new(42), Ok(limit));
        assert_eq!(ReplayLimit::try_from(42), Ok(limit));
        assert_eq!(u64::from(limit), 42);
        assert_eq!(limit.to_string(), "42");
    }

    #[test]
    fn rejects_zero_with_typed_error() {
        let error = ReplayLimit::try_new(0).unwrap_err();

        assert_eq!(error.value(), 0);
        assert_eq!(error.to_string(), "replay limit must be greater than zero, got 0");
        let _: &dyn std::error::Error = &error;
    }
}
