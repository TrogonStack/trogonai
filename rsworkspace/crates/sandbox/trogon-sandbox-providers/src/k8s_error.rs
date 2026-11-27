use crate::{K8sNetworkPolicyError, K8sOpsError, K8sOwnershipError, K8sProviderError};

/// Failures of the Kubernetes adapters.
#[derive(Debug, thiserror::Error)]
pub enum K8sError {
    #[error(transparent)]
    NetworkPolicy(#[from] K8sNetworkPolicyError),
    #[error(transparent)]
    Ownership(#[from] K8sOwnershipError),
    #[error(transparent)]
    Ops(#[from] K8sOpsError),
    #[error(transparent)]
    Provider(#[from] K8sProviderError),
}
