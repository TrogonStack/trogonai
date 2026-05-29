use std::fmt;

/// Errors from context throttle key construction or bucket refill.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextThrottleError {
    MalformedKey { field: &'static str },
    ClockSkew,
}

impl fmt::Display for ContextThrottleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedKey { field } => {
                write!(f, "context throttle key field {field} must be non-empty")
            }
            Self::ClockSkew => write!(f, "monotonic clock moved backward"),
        }
    }
}

impl std::error::Error for ContextThrottleError {}
