use std::io;

use trogon_sandbox_auth::ApiKeyStoreError;
use trogon_sandbox_observability::ObservabilityError;
use trogon_sandbox_protocol::DispatchSignerConfigError;

use crate::{HostKeyError, ServeError};

/// Why the gateway process stopped.
#[derive(Debug, thiserror::Error)]
pub enum GatewayRunError {
    #[error("failed to set up observability: {0}")]
    Observability(#[from] ObservabilityError),
    #[error("failed to connect to NATS: {0}")]
    Connect(#[from] async_nats::ConnectError),
    #[error("failed to open the API key store: {0}")]
    ApiKeys(#[from] ApiKeyStoreError),
    #[error("dispatch signing is misconfigured: {0}")]
    Signer(#[from] DispatchSignerConfigError),
    #[error("host key: {0}")]
    HostKey(#[from] HostKeyError),
    #[error("failed to bind the SSH listener: {0}")]
    Bind(#[source] io::Error),
    #[error("ssh server failed: {0}")]
    Serve(#[from] ServeError),
    #[error("failed to install the shutdown signal handler: {0}")]
    Signal(#[source] io::Error),
}
