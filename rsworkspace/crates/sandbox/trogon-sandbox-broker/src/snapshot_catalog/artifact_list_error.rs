use super::snapshot_catalog_error::SnapshotCatalogError;

/// Why listing registry artifacts failed.
#[derive(Debug, thiserror::Error)]
pub enum ArtifactListError {
    #[error("snapshot catalog unavailable for the artifact listing: {0}")]
    Catalog(#[from] SnapshotCatalogError),
}
