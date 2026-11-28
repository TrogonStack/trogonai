use trogon_sandbox_auth::ApiKeyStoreError;
use trogon_sandbox_observability::ObservabilityError;
use trogon_sandbox_protocol::NatsConnectError;

use crate::{PreviewConfigError, SessionStoreError};

/// Failures while running the gateway.
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("failed to bind the gateway listener: {0}")]
    Bind(#[source] std::io::Error),
    #[error("gateway server failed: {0}")]
    Serve(#[source] std::io::Error),
    #[error("failed to install the shutdown signal handler: {0}")]
    Signal(#[source] std::io::Error),
    #[error("failed to load the TLS certificate or key: {0}")]
    Tls(#[source] std::io::Error),
    #[error("failed to set up observability: {0}")]
    Observability(#[from] ObservabilityError),
    #[error("failed to connect to NATS: {0}")]
    Connect(#[from] NatsConnectError),
    #[error("API key store unavailable: {0}")]
    ApiKeyStore(#[from] ApiKeyStoreError),
    #[error("session store unavailable: {0}")]
    SessionStore(#[from] SessionStoreError),
    #[error("invalid preview configuration: {0}")]
    Preview(#[from] PreviewConfigError),
}
