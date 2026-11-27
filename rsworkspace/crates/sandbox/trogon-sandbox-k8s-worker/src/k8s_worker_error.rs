use trogon_sandbox_contract::{LeaseError, LeaseId, SandboxIdError, SandboxSpecError, TokenError};
use trogon_sandbox_cosign::SnapshotPullError;
use trogon_sandbox_protocol::{AuthorityResourceIdError, PrivilegedUnsupportedError};
use trogon_sandbox_providers::{K8sOpsError, K8sProviderError, LeaseSpecError};

use crate::{SnapshotTransferError, VolumeSyncError, WarmPoolError};

/// Why the Kubernetes worker could not carry out a lease operation.
#[derive(Debug, thiserror::Error)]
pub enum K8sWorkerError {
    #[error(transparent)]
    Provider(#[from] K8sProviderError),
    #[error(transparent)]
    Ops(#[from] K8sOpsError),
    #[error(transparent)]
    Snapshot(#[from] SnapshotTransferError),
    #[error(transparent)]
    Pull(#[from] SnapshotPullError),
    #[error(transparent)]
    Volume(#[from] VolumeSyncError),
    #[error(transparent)]
    WarmPool(#[from] WarmPoolError),
    #[error(transparent)]
    Lease(#[from] LeaseError),
    #[error(transparent)]
    LeaseSpec(#[from] LeaseSpecError),
    #[error(transparent)]
    PrivilegedUnsupported(#[from] PrivilegedUnsupportedError),
    #[error(transparent)]
    Spec(#[from] SandboxSpecError),
    #[error(transparent)]
    SandboxId(#[from] SandboxIdError),
    #[error(transparent)]
    WorkerId(#[from] TokenError),
    #[error(transparent)]
    ResourceId(#[from] AuthorityResourceIdError),
    #[error("no active lease for {0}")]
    NoActiveLease(LeaseId),
    #[error("Kubernetes lease ID is already held by this worker")]
    LeaseAlreadyHeld,
    #[error("Kubernetes cleanup must settle before this lease can be launched again")]
    CleanupPending,
    #[error("warm claim has no confirmed Kubernetes pod UID")]
    WarmClaimWithoutReceipt,
    #[error("created Kubernetes Job has no ownership receipt")]
    JobWithoutReceipt,
    #[error("command exceeded its deadline; termination is unconfirmed")]
    TerminationUnconfirmed,
    #[error("timeout signal was accepted, but native command completion is unconfirmed")]
    CompletionUnconfirmed,
    #[error("{action} failed (exit {exit_code}): {stderr}")]
    CommandFailed {
        action: &'static str,
        exit_code: i32,
        stderr: String,
    },
}
