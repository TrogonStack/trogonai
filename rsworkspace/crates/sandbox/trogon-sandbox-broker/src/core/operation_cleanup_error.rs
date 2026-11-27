use trogon_sandbox_contract::SandboxIdError;

use super::dispatch_transaction_error::DispatchTransactionError;
use crate::active_lease::ActiveLeaseStoreError;
use crate::command_launch::CommandLaunchError;
use crate::command_logs::CommandLogError;
use crate::drive_store::DriveStoreError;
use crate::named_lease::NamedLeaseStoreError;
use crate::operations::OperationsError;
use crate::quota::QuotaStoreError;
use crate::resolved_digest::ResolvedDigestError;

/// Why cleaning up the resources a durable operation owns failed.
#[derive(Debug, thiserror::Error)]
pub(super) enum OperationCleanupError {
    #[error("cleanup intent does not match its original allocation")]
    IntentMismatch,
    #[error("owned allocation has no retained active identity")]
    NoActiveIdentity,
    #[error("owned allocation identity changed before cleanup")]
    IdentityChanged,
    #[error("owned allocation was replaced before cleanup")]
    Replaced,
    #[error("targeted allocation has no retained attempt record")]
    NoAttemptRecord,
    #[error("provider does not offer acknowledged owned-resource cleanup")]
    UnsupportedProvider,
    #[error("owned allocation changed while recording cleanup intent")]
    IntentRecordingLost,
    #[error("worker did not confirm the exact owned allocation was absent")]
    WorkerUnconfirmed,
    #[error("owned allocation changed before cleanup bookkeeping")]
    BookkeepingChanged,
    #[error("owned allocation changed while sealing its release")]
    SealingLost,
    #[error("canceled operation has no create request")]
    NoCreateRequest,
    #[error("canceled create has no exact quota reservation")]
    NoReservation,
    #[error("canceled create active identity differs from its original allocation")]
    ActiveIdentityDiffers,
    #[error("canceled create assignment differs from its stored dispatch")]
    AssignmentDiffers,
    #[error("create allocation claim was lost before its provenance was retained")]
    ProvenanceClaimLost,
    #[error("canceled create lost its allocation claim")]
    AllocationClaimLost,
    #[error("{message}")]
    Bus { message: String },
    #[error("{message}")]
    SnapshotPin { message: String },
    #[error(transparent)]
    SandboxId(#[from] SandboxIdError),
    #[error(transparent)]
    Dispatch(#[from] DispatchTransactionError),
    #[error(transparent)]
    ActiveLease(#[from] ActiveLeaseStoreError),
    #[error(transparent)]
    Operations(#[from] OperationsError),
    #[error(transparent)]
    NamedLease(#[from] NamedLeaseStoreError),
    #[error(transparent)]
    Quota(#[from] QuotaStoreError),
    #[error(transparent)]
    Drive(#[from] DriveStoreError),
    #[error(transparent)]
    ResolvedDigest(#[from] ResolvedDigestError),
    #[error(transparent)]
    CommandLaunch(#[from] CommandLaunchError),
    #[error(transparent)]
    CommandLog(#[from] CommandLogError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl OperationCleanupError {
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
