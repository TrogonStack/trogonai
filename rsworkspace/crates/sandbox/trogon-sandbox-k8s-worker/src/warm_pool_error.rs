use trogon_sandbox_providers::{K8sOpsError, K8sProviderError};

use crate::SnapshotTransferError;

/// Failures of a warm pool claim or reconcile pass.
#[derive(Debug, thiserror::Error)]
pub enum WarmPoolError {
    #[error(transparent)]
    Provider(#[from] K8sProviderError),
    #[error(transparent)]
    Ops(#[from] K8sOpsError),
    #[error(transparent)]
    Snapshot(#[from] SnapshotTransferError),
    #[error("warm claim has no confirmed pod UID")]
    ClaimWithoutPod,
}
