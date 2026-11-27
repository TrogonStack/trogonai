use super::dispatch_transaction_error::DispatchTransactionError;
use crate::active_lease::ActiveLeaseStoreError;
use crate::drive_store::DriveStoreError;
use crate::idempotency::IdempotencyStoreError;
use crate::operations::OperationsError;
use crate::quota::QuotaStoreError;
use crate::resolved_digest::ResolvedDigestError;

/// Why settling the leftovers of a durable create failed.
#[derive(Debug, thiserror::Error)]
pub(super) enum CreateOperationError {
    #[error("attempt missing")]
    MissingAttempt,
    #[error("legacy create quota has no replay receipt; manual accounting reconciliation is required")]
    NoReplayReceipt,
    #[error("durable create quota receipt does not match the operation")]
    ReceiptMismatchesOperation,
    #[error("durable create quota receipt does not match the active lease")]
    ReceiptMismatchesLease,
    #[error("operation claim was lost while reconciling quota")]
    ClaimLost,
    #[error("{message}")]
    SnapshotPin { message: String },
    #[error(transparent)]
    Dispatch(#[from] DispatchTransactionError),
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
}

impl CreateOperationError {
    pub(super) fn snapshot_pin(error: impl std::fmt::Display) -> Self {
        Self::SnapshotPin {
            message: error.to_string(),
        }
    }
}
