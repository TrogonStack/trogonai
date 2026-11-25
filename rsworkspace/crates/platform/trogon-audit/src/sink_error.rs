use crate::failure_reason::FailureReason;

/// Why a sink did not store a record. A record that was not acknowledged is
/// treated as not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SinkError {
    #[error("audit stream is unreachable")]
    Unreachable,
    #[error("audit publish was not acknowledged in time")]
    Timeout,
    #[error("audit stream is full and refuses new records")]
    StreamFull,
}

impl SinkError {
    pub fn reason(self) -> FailureReason {
        match self {
            Self::Unreachable => FailureReason::Unreachable,
            Self::Timeout => FailureReason::Timeout,
            Self::StreamFull => FailureReason::StreamFull,
        }
    }
}

#[cfg(test)]
mod tests;
