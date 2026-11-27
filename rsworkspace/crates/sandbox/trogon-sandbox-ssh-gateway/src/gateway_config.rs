use std::num::NonZeroUsize;
use std::sync::Arc;

use trogon_sandbox_auth::ApiKeyStore;

use crate::{SessionKeyMint, SshTunnelBackend};

/// What the gateway needs to serve connections.
#[derive(Clone)]
pub struct GatewayConfig {
    /// Resolves the bearer a client presents as its SSH password.
    pub api_keys: Arc<dyn ApiKeyStore>,
    /// How the gateway reaches the worker holding a lease.
    pub tunnel: Arc<dyn SshTunnelBackend>,
    /// Registers sessions with the credential broker, when one is reachable.
    pub key_mint: Option<Arc<SessionKeyMint>>,
    /// Most SSH connections served at once.
    pub max_connections: NonZeroUsize,
}
