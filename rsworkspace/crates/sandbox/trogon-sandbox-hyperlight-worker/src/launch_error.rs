use trogon_sandbox_observability::ObservabilityError;
use trogon_sandbox_oci::RegistryHostError;
use trogon_sandbox_protocol::{AuthorityModeError, NatsConnectError};

use crate::{HyperlightWorkerError, RuntimeError};

/// Why the worker could not start or stopped abnormally.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    /// `SANDBOX_WORKER_AUTHORITY` is not a mode.
    #[error(transparent)]
    Authority(#[from] AuthorityModeError),
    /// Telemetry could not start.
    #[error(transparent)]
    Telemetry(#[from] ObservabilityError),
    /// An insecure registry is not a valid host.
    #[error("invalid insecure registry: {0}")]
    Registry(#[from] RegistryHostError),
    /// The runtime could not be built.
    #[error("failed to initialise the Hyperlight runtime: {0}")]
    Runtime(#[from] RuntimeError),
    /// The worker identity is not a valid token.
    #[error("invalid worker id: {0}")]
    WorkerId(#[from] trogon_sandbox_contract::TokenError),
    /// NATS is unreachable.
    #[error(transparent)]
    Connect(#[from] NatsConnectError),
    /// The worker failed while serving.
    #[error(transparent)]
    Worker(#[from] HyperlightWorkerError),
}
