use trogon_sandbox_oci::RegistryHostError;

use crate::SnapshotBuilderError;

/// Why the builder process could not serve.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error(transparent)]
    Registry(#[from] RegistryHostError),
    #[error("failed to register the builder metrics: {0}")]
    Metrics(#[from] prometheus::Error),
    #[error(transparent)]
    Builder(#[from] SnapshotBuilderError),
}
