use trogon_sandbox_contract::{LeaseError, NativeSnapshotIdError, SandboxIdError};
use trogon_sandbox_protocol::{AttemptLedgerError, NativeOptionsError, PrivilegedUnsupportedError, ProviderError};

use super::LeaseLifetimeError;
use crate::SnapshotLifecycleError;
use crate::api::{ApiError, E2bConfigError, NativeAccessTokenError};
use crate::assignments::AssignmentsError;
use crate::envd::EnvdTargetError;
use crate::lease_cleanup::LeaseCleanupError;
use crate::snapshot_custody::SnapshotCustodyError;
use crate::snapshots::{SnapshotManifestError, SnapshotManifestsError, SnapshotReferenceError};

/// Why the E2B provider could not serve a lease, cleanup or snapshot call.
#[derive(Debug, thiserror::Error)]
pub enum E2bProviderError {
    #[error(transparent)]
    NativeOptions(#[from] NativeOptionsError),
    #[error(transparent)]
    Privileged(#[from] PrivilegedUnsupportedError),
    #[error(transparent)]
    Lease(#[from] LeaseError),
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    SnapshotId(#[from] NativeSnapshotIdError),
    #[error("E2B named the sandbox with an id the protocol refuses: {0}")]
    SandboxId(#[from] SandboxIdError),
    #[error(transparent)]
    Assignments(#[from] AssignmentsError),
    #[error(transparent)]
    Cleanup(#[from] LeaseCleanupError),
    #[error(transparent)]
    Ledger(#[from] AttemptLedgerError),
    #[error(transparent)]
    Envd(#[from] EnvdTargetError),
    #[error(transparent)]
    Manifest(#[from] SnapshotManifestError),
    #[error(transparent)]
    Manifests(#[from] SnapshotManifestsError),
    #[error(transparent)]
    Reference(#[from] SnapshotReferenceError),
    #[error(transparent)]
    SnapshotLifecycle(#[from] SnapshotLifecycleError),
    #[error(transparent)]
    Custody(#[from] SnapshotCustodyError),
    #[error(transparent)]
    Lifetime(#[from] LeaseLifetimeError),
    #[error(transparent)]
    Config(#[from] E2bConfigError),
    #[error(transparent)]
    Token(#[from] NativeAccessTokenError),
    #[error("this lease still holds a native resource; settle cleanup before reallocating")]
    AlreadyHeld,
    #[error("RemoteMount workspaces are not supported on the E2B provider: E2B cannot bind a host path into a microVM")]
    MountsUnsupported,
    #[error("provider is already bound to another worker")]
    BoundToAnotherWorker,
    #[error("E2B needs a template name")]
    TemplateRequired,
    #[error("native snapshot deletion has started")]
    SnapshotDeletionStarted,
    #[error("native snapshot metadata is not owned by this tenant")]
    SnapshotNotOwned,
    #[error("allocation lost its dispatch ledger")]
    LedgerMissing,
    #[error("sandbox delete was acknowledged but exact native absence is not confirmed")]
    DeleteUnconfirmed,
    #[error("{restore}; restored sandbox cleanup failed: {cleanup}")]
    RestoreCleanupFailed { restore: LeaseError, cleanup: ApiError },
    #[error("{source}; owned cleanup claim remains unresolved: {journal}")]
    ClaimUnresolved {
        #[source]
        source: Box<E2bProviderError>,
        journal: LeaseCleanupError,
    },
}

impl From<E2bProviderError> for ProviderError {
    fn from(error: E2bProviderError) -> Self {
        match error {
            E2bProviderError::NativeOptions(error) => Self::NativeOptions(error),
            E2bProviderError::Privileged(error) => Self::PrivilegedUnsupported(error),
            other => Self::backend(other),
        }
    }
}
