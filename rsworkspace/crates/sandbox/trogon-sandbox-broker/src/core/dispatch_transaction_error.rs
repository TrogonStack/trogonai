use trogon_sandbox_contract::LeaseError;

use crate::active_lease::ActiveLeaseStoreError;
use crate::drive_store::DriveStoreError;
use crate::idempotency::IdempotencyStoreError;
use crate::operations::OperationsError;
use crate::quota::QuotaStoreError;
use crate::resolved_digest::ResolvedDigestError;

/// Why a step of a targeted dispatch, or of tearing one down, failed.
#[derive(Debug, thiserror::Error)]
pub enum DispatchTransactionError {
    #[error("lease has no dispatch attempt")]
    MissingAttempt,
    #[error("target worker missing")]
    MissingTargetWorker,
    #[error("lease changed during update")]
    LeaseChanged,
    #[error("dispatch preparation was fenced by cleanup")]
    PreparationFenced,
    #[error("credential policy commit failed: {message}")]
    CredentialPolicy { message: String },
    #[error("operation claim was lost before the dispatch was stored")]
    OperationClaimLost,
    #[error("cleanup attempt identity changed")]
    CleanupIdentityChanged,
    #[error("cleanup lost its attempt transition")]
    CleanupTransitionLost,
    #[error("cleanup completion lost its attempt transition")]
    CleanupCompletionLost,
    #[error("provider did not confirm exact attempt teardown and intent")]
    TeardownUnconfirmed,
    #[error("{message}")]
    Bus { message: String },
    #[error("{message}")]
    SnapshotPin { message: String },
    #[error(transparent)]
    Lease(#[from] LeaseError),
    #[error(transparent)]
    ActiveLease(#[from] ActiveLeaseStoreError),
    #[error(transparent)]
    Idempotency(#[from] IdempotencyStoreError),
    #[error(transparent)]
    Drive(#[from] DriveStoreError),
    #[error(transparent)]
    Quota(#[from] QuotaStoreError),
    #[error(transparent)]
    ResolvedDigest(#[from] ResolvedDigestError),
    #[error(transparent)]
    Operations(#[from] OperationsError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl DispatchTransactionError {
    pub(super) fn bus(error: impl std::fmt::Display) -> Self {
        Self::Bus {
            message: error.to_string(),
        }
    }

    pub(super) fn snapshot_pin(error: impl std::fmt::Display) -> Self {
        Self::SnapshotPin {
            message: error.to_string(),
        }
    }
}
