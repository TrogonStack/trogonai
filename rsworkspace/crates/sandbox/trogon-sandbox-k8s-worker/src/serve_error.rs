use async_nats::ConnectError;
use trogon_sandbox_contract::TokenError;
use trogon_sandbox_cosign::CosignError;
use trogon_sandbox_oci::RegistryHostError;
use trogon_sandbox_providers::K8sProviderError;

use crate::{K8sWorkerError, WorkerError};

/// Why the worker process could not serve.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The NATS server could not be reached.
    #[error("failed to connect to NATS at {url}: {source}")]
    Connect {
        url: String,
        #[source]
        source: ConnectError,
    },
    /// The worker identity was rejected as a worker id.
    #[error(transparent)]
    WorkerId(#[from] TokenError),
    /// An insecure registry is not a registry host.
    #[error(transparent)]
    Registry(#[from] RegistryHostError),
    /// The Kubernetes client could not be built.
    #[error("failed to build the Kubernetes provider: {0}")]
    Provider(#[from] K8sProviderError),
    /// The Cosign public key could not be loaded.
    #[error("failed to load the Cosign public key: {0}")]
    Cosign(#[from] CosignError),
    /// Leases could not be recovered from the cluster.
    #[error("failed to recover leases from the cluster: {0}")]
    Recovery(#[from] K8sWorkerError),
    /// The worker loop failed.
    #[error(transparent)]
    Worker(#[from] WorkerError),
}
