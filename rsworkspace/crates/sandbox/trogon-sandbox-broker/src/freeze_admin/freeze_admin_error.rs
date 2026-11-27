use crate::active_lease::ActiveLeaseStoreError;
use crate::idempotency::IdempotencyStoreError;

/// Why resolving an unsettled freeze failed.
#[derive(Debug, thiserror::Error)]
pub enum FreezeAdminError {
    #[error("active lease store failed: {0}")]
    ActiveLease(#[from] ActiveLeaseStoreError),
    #[error("idempotency journal failed: {0}")]
    Idempotency(#[from] IdempotencyStoreError),
}
