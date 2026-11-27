use crate::snapshot_accounting::SnapshotAccountingError;
use crate::snapshot_catalog::SnapshotCatalogError;
use crate::snapshot_live::SnapshotLiveError;

/// Why opening the catalog sweeper's stores failed.
#[derive(Debug, thiserror::Error)]
pub enum CatalogSweeperOpenError {
    #[error("snapshot accounting store unavailable: {0}")]
    Accounting(#[from] SnapshotAccountingError),
    #[error("snapshot live store unavailable: {0}")]
    Live(#[from] SnapshotLiveError),
    #[error("snapshot catalog unavailable: {0}")]
    Catalog(#[from] SnapshotCatalogError),
}
