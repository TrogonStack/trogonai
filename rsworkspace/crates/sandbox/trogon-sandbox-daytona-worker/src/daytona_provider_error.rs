use trogon_sandbox_contract::{LeaseError, SandboxIdError};
use trogon_sandbox_protocol::{NativeOptionsError, PrivilegedUnsupportedError, ProviderError};

use crate::{DaytonaApiError, SandboxState};

/// Why the Daytona provider could not serve a lease or cleanup call.
#[derive(Debug, thiserror::Error)]
pub enum DaytonaProviderError {
    #[error(transparent)]
    NativeOptions(#[from] NativeOptionsError),
    #[error(transparent)]
    Privileged(#[from] PrivilegedUnsupportedError),
    #[error(transparent)]
    Lease(#[from] LeaseError),
    #[error(transparent)]
    Api(#[from] DaytonaApiError),
    #[error("Daytona named the sandbox with an id the protocol refuses: {0}")]
    SandboxId(#[from] SandboxIdError),
    #[error("this lease still holds a native resource; settle cleanup before reallocating")]
    AlreadyHeld,
    #[error("RemoteMount workspaces are not supported: Daytona cannot bind a host path into a sandbox")]
    MountsUnsupported,
    #[error("the sandbox reached state {state:?} and cannot recover{}", reason_suffix(.reason))]
    SandboxFailed {
        state: SandboxState,
        reason: Option<String>,
    },
    #[error("the sandbox did not reach {target:?} in time; Daytona last reported {state:?}")]
    SettleTimedOut { state: SandboxState, target: SandboxState },
    #[error("provider is already bound to another worker")]
    BoundToAnotherWorker,
}

fn reason_suffix(reason: &Option<String>) -> String {
    reason.as_ref().map_or_else(String::new, |reason| format!(": {reason}"))
}

impl From<DaytonaProviderError> for ProviderError {
    fn from(error: DaytonaProviderError) -> Self {
        match error {
            DaytonaProviderError::NativeOptions(error) => Self::NativeOptions(error),
            DaytonaProviderError::Privileged(error) => Self::PrivilegedUnsupported(error),
            other => Self::backend(other),
        }
    }
}
