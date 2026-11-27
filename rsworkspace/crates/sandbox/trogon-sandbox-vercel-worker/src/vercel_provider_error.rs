use trogon_sandbox_contract::{LeaseError, SandboxIdError};
use trogon_sandbox_protocol::{NativeOptionsError, PrivilegedUnsupportedError, ProviderError};

use crate::VercelApiError;

/// Why the Vercel provider could not serve a lease or cleanup call.
#[derive(Debug, thiserror::Error)]
pub enum VercelProviderError {
    #[error(transparent)]
    NativeOptions(#[from] NativeOptionsError),
    #[error(transparent)]
    Privileged(#[from] PrivilegedUnsupportedError),
    #[error(transparent)]
    Lease(#[from] LeaseError),
    #[error(transparent)]
    Api(#[from] VercelApiError),
    #[error("Vercel named the sandbox with an id the protocol refuses: {0}")]
    SandboxId(#[from] SandboxIdError),
    #[error("this lease still holds a native resource; settle cleanup before reallocating")]
    AlreadyHeld,
    #[error(
        "RemoteMount workspaces are not supported on the Vercel provider: Vercel cannot bind a host path into a microVM"
    )]
    MountsUnsupported,
    #[error("provider is already bound to another worker")]
    BoundToAnotherWorker,
}

impl From<VercelProviderError> for ProviderError {
    fn from(error: VercelProviderError) -> Self {
        match error {
            VercelProviderError::NativeOptions(error) => Self::NativeOptions(error),
            VercelProviderError::Privileged(error) => Self::PrivilegedUnsupported(error),
            other => Self::backend(other),
        }
    }
}
