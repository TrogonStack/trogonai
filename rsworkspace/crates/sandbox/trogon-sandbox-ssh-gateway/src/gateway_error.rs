use trogon_sandbox_auth::ApiKeyStoreError;
use trogon_sandbox_contract::TerminalDimensionError;
use trogon_sandbox_credential_client::ClientError;

use crate::{KeyError, TunnelError};

/// Why the gateway ended an SSH connection.
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("ssh protocol: {0}")]
    Ssh(#[from] russh::Error),
    #[error("invalid SSH user: expected <lease-id>@gw")]
    InvalidUser,
    #[error("api key store: {0}")]
    ApiKeys(#[from] ApiKeyStoreError),
    #[error("session key: {0}")]
    SessionKey(#[from] KeyError),
    #[error("credential broker: {0}")]
    Broker(#[from] ClientError),
    #[error("attach: {0}")]
    Attach(#[source] TunnelError),
    #[error("tunnel: {0}")]
    Tunnel(#[from] TunnelError),
    #[error("terminal size: {0}")]
    Dimension(#[from] TerminalDimensionError),
    #[error("the connection has no attached session")]
    NotAttached,
}
