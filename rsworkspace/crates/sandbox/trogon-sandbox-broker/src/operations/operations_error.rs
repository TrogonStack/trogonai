use super::operation_store_error::OperationStoreError;

/// A failure of the durable operation ledger.
#[derive(Debug, thiserror::Error)]
pub enum OperationsError {
    #[error(transparent)]
    Store(#[from] OperationStoreError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("owned cleanup record identity mismatch")]
    CleanupIdentityMismatch,
    #[error("owned cleanup intent vanished")]
    CleanupIntentVanished,
    #[error("owned cleanup intent is missing")]
    CleanupIntentMissing,
    #[error("owned cleanup allocation identity changed")]
    CleanupAllocationChanged,
    #[error("owned cleanup phase exhausted CAS retries")]
    CleanupCasExhausted,
    #[error("operation allocation identity changed")]
    AllocationChanged,
    #[error("operation allocation does not match its original authority")]
    AllocationAuthorityMismatch,
    #[error("operation allocation has no retained create authority")]
    AllocationWithoutCreate,
    #[error("operation allocation attempt differs from its original authority")]
    AllocationAttemptMismatch,
    #[error("operation allocation assignment differs from its original dispatch")]
    AllocationAssignmentMismatch,
    #[error("targeted allocation has no retained dispatch authority")]
    AllocationWithoutDispatch,
    #[error("operation allocation exhausted CAS retries")]
    AllocationCasExhausted,
    #[error("operation record identity mismatch")]
    RecordIdentityMismatch,
    #[error("operation cancellation identity mismatch")]
    CancellationIdentityMismatch,
    #[error("operation cancellation intent vanished")]
    CancellationVanished,
    #[error("operation does not support cancellation")]
    CancellationUnsupported,
    #[error("operation cancellation intent exhausted CAS retries")]
    CancellationCasExhausted,
    #[error("operation vanished after creation")]
    VanishedAfterCreation,
    #[error("operation record vanished while it was created")]
    VanishedDuringCreate,
    #[error("lease marker update exhausted CAS retries")]
    LeaseMarkerCasExhausted,
    #[error("operation claim generation exhausted")]
    ClaimGenerationExhausted,
    #[error("operation update exhausted CAS retries")]
    UpdateCasExhausted,
}
