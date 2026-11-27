use trogon_sandbox_contract::SnapshotAccountingRefusal;

use crate::snapshot_catalog::ArtifactDeleteError;
use crate::snapshot_live::SnapshotLiveError;

/// Why removing a tenant's snapshot failed.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotRemovalError {
    #[error("snapshot deletion claim refused: {0:?}")]
    ClaimRefused(SnapshotAccountingRefusal),
    #[error("snapshot deletion reconciliation refused: {0:?}")]
    ReconciliationRefused(SnapshotAccountingRefusal),
    #[error("snapshot deletion completion remains pending: {0:?}")]
    CompletionPending(SnapshotAccountingRefusal),
    #[error("legacy snapshot ownership requires reconciliation")]
    LegacyOwnership,
    #[error("legacy shared snapshot repository requires ownership reconciliation")]
    LegacyRepository,
    #[error("snapshot deletion requires a durable host accounting claim")]
    ClaimRequired,
    #[error("snapshot accounting unavailable")]
    AccountingUnavailable,
    #[error("snapshot live projection unavailable")]
    LiveUnavailable,
    #[error(transparent)]
    Live(#[from] SnapshotLiveError),
    #[error(transparent)]
    Delete(#[from] ArtifactDeleteError),
}
