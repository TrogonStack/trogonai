use trogon_sandbox_broker::snapshot_tag::SnapshotTagStoreError;
use trogon_sandbox_contract::SnapshotAccountingRefusal;

/// Why one recovery attempt left a published snapshot's tag unmoved.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RecoveryAttemptError {
    #[error("snapshot accounting unconfirmed: {0:?}")]
    Unconfirmed(SnapshotAccountingRefusal),
    #[error("{0}")]
    Tag(#[from] SnapshotTagStoreError),
}
