use trogon_sandbox_contract::LeaseId;

use super::dispatch_transaction_error::DispatchTransactionError;
use crate::active_lease::ActiveLeaseStoreError;
use crate::drive_store::DriveStoreError;
use crate::named_lease::NamedLeaseStoreError;
use crate::pending_purge::PendingPurgeError;
use crate::pending_release::PendingReleaseError;
use crate::quota::QuotaStoreError;

/// Why releasing or purging a lease failed.
#[derive(Debug, thiserror::Error)]
pub enum LeaseReleaseError {
    #[error("lease {} not found", .0.as_str())]
    NotFound(LeaseId),
    #[error("drive claim release: {0}")]
    DriveClaimRelease(#[source] DriveStoreError),
    #[error("{message}")]
    Bus { message: String },
    #[error(transparent)]
    Dispatch(#[from] DispatchTransactionError),
    #[error(transparent)]
    ActiveLease(#[from] ActiveLeaseStoreError),
    #[error(transparent)]
    Quota(#[from] QuotaStoreError),
    #[error(transparent)]
    PendingPurge(#[from] PendingPurgeError),
    #[error(transparent)]
    PendingRelease(#[from] PendingReleaseError),
    #[error(transparent)]
    NamedLease(#[from] NamedLeaseStoreError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl LeaseReleaseError {
    pub(super) fn bus(error: impl std::fmt::Display) -> Self {
        Self::Bus {
            message: error.to_string(),
        }
    }
}
