use trogon_sandbox_contract::LeaseError;

use crate::DaytonaApiError;

/// Why running a command, or reading what it left behind, failed.
#[derive(Debug, thiserror::Error)]
pub enum DaytonaExecError {
    #[error(transparent)]
    Api(#[from] DaytonaApiError),
    #[error("command outcome is unknown: the deadline receipt could not be read: {0}")]
    ReceiptUnreadable(DaytonaApiError),
    #[error("command outcome is unknown: invalid deadline receipt")]
    ReceiptInvalid,
    #[error("command deadline requires the timeout utility in the image")]
    TimeoutUnavailable,
    #[error("command {stream} could not be read")]
    StreamUnreadable { stream: &'static str },
    #[error("Daytona returned no command id")]
    NoCommandId,
    #[error("ordinary command signals require a configured verified runtime")]
    SignalNeedsRuntime,
    #[error("command outcome is unknown: the ordinary command runtime did not confirm its state")]
    OrdinaryUnconfirmed,
    #[error("ordinary command runtime: {0}")]
    Runtime(#[from] crate::OrdinaryCommandRuntimeError),
}

impl DaytonaExecError {
    /// The reply a caller sees: a request the image cannot honor is its own mistake, the rest are the backend's.
    pub(crate) fn into_lease_error(self) -> LeaseError {
        match self {
            Self::TimeoutUnavailable => LeaseError::InvalidSpec {
                reason: self.to_string(),
            },
            other => LeaseError::Backend {
                message: other.to_string(),
            },
        }
    }
}
