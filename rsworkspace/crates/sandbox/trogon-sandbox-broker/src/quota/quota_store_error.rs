use async_nats::jetstream::{self, kv};
use thiserror::Error;
use trogon_sandbox_contract::{LeaseError, SnapshotAccountingRefusal};

/// Why a [`QuotaStore`](super::QuotaStore) operation failed.
#[derive(Debug, Error)]
pub enum QuotaStoreError {
    #[error("atomic dispatch accounting is unavailable")]
    AttemptAccountingUnavailable,
    #[error("quota attempt identity was reused")]
    AttemptIdentityReused,
    #[error("held quota record is missing")]
    HeldRecordMissing,
    #[error("attempt CPU accounting is inconsistent")]
    AttemptCpuInconsistent,
    #[error("attempt wall accounting is inconsistent")]
    AttemptWallInconsistent,
    #[error("attempt active accounting is inconsistent")]
    AttemptActiveInconsistent,
    #[error("attempt prewarm CPU accounting is inconsistent")]
    AttemptPrewarmCpuInconsistent,
    #[error("attempt prewarm count is inconsistent")]
    AttemptPrewarmCountInconsistent,
    #[error("quota budget disappeared during settlement")]
    BudgetDisappeared,
    #[error("quota receipt disappeared during settlement")]
    ReceiptDisappeared,
    #[error("quota receipt advanced while the budget mutation remained pending")]
    ReceiptAdvanced,
    #[error("quota store CAS failed after max retries")]
    CasRetriesExhausted,
    #[error("quota store CAS failed after max retries on release")]
    ReleaseCasRetriesExhausted,
    #[error("quota receipt CAS retries exhausted")]
    ReceiptCasRetriesExhausted,
    #[error("quota settlement CAS retries exhausted")]
    SettlementCasRetriesExhausted,
    #[error("legacy snapshot inventory exceeds reconciliation bound")]
    LegacyInventoryTooLarge,
    #[error("legacy snapshot usage requires reconciliation")]
    LegacyUsageRequiresReconciliation,
    #[error("snapshot legacy observation: {0:?}")]
    LegacyObservation(SnapshotAccountingRefusal),
    #[error("snapshot legacy quarantine: {0:?}")]
    LegacyQuarantine(SnapshotAccountingRefusal),
    #[error("snapshot accounting: {0:?}")]
    SnapshotAccounting(SnapshotAccountingRefusal),
    #[error("snapshot store: {0}")]
    SnapshotStore(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error(transparent)]
    Lease(#[from] LeaseError),
    #[error("quota record serialization: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("quota bucket: {0}")]
    Bucket(#[from] jetstream::context::CreateKeyValueError),
    #[error("quota entry read: {0}")]
    Entry(#[from] kv::EntryError),
    #[error("quota entry create: {0}")]
    Create(#[from] kv::CreateError),
    #[error("quota entry update: {0}")]
    Update(#[from] kv::UpdateError),
    #[error("quota key listing: {0}")]
    History(#[from] kv::HistoryError),
    #[error("quota key stream: {0}")]
    Watcher(#[from] kv::WatcherError),
}

impl QuotaStoreError {
    /// Wraps a failure from one of the snapshot stores a quota store opens.
    pub(crate) fn snapshot_store(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self::SnapshotStore(error.into())
    }
}
