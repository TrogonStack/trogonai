use async_nats::ConnectError;

use crate::{KubeVirtProviderError, KubeVirtWorkerError};

/// Why the worker process could not serve.
#[derive(Debug, thiserror::Error)]
pub enum ServeWorkerError {
    #[error("failed to connect to NATS at {url}: {source}")]
    Connect {
        url: String,
        #[source]
        source: ConnectError,
    },
    #[error("failed to build the kubernetes client: {0}")]
    Kube(#[from] kube::Error),
    #[error("failed to load the cosign public key: {0}")]
    Cosign(#[from] trogon_sandbox_cosign::CosignError),
    #[error("worker authority mode is invalid: {0}")]
    Authority(#[from] trogon_sandbox_protocol::AuthorityModeError),
    #[error(transparent)]
    Provider(#[from] KubeVirtProviderError),
    #[error(transparent)]
    Worker(#[from] KubeVirtWorkerError),
}
