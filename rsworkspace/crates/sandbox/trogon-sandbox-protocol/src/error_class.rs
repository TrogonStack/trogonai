/// How a caller should react to an error.
///
/// `retryable`: repeating the same request after a backoff may succeed.
/// `outcome_unknown`: the original request may already have been applied, so
/// a retry must reuse the idempotency key or check state first.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ErrorClass {
    pub retryable: bool,
    pub outcome_unknown: bool,
    pub hint: &'static str,
}

impl ErrorClass {
    /// Repeating the request cannot succeed until the caller changes something.
    pub const fn permanent(hint: &'static str) -> Self {
        Self {
            retryable: false,
            outcome_unknown: false,
            hint,
        }
    }

    /// Repeating the request after a backoff may succeed.
    pub const fn transient(hint: &'static str) -> Self {
        Self {
            retryable: true,
            outcome_unknown: false,
            hint,
        }
    }

    /// The request may already have been applied, so a retry must check state first.
    pub const fn unknown_outcome(hint: &'static str) -> Self {
        Self {
            retryable: true,
            outcome_unknown: true,
            hint,
        }
    }
}
