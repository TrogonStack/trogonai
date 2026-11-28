//! Signing of lease dispatches, so workers can tell a dispatch came from a
//! broker holding the signing key and was not altered on the way.

use std::sync::Arc;

use bytes::Bytes;
use tracing::{info, warn};
use trogon_sandbox_protocol::{DispatchSignError, DispatchSigner, OP_LEASE_DISPATCH};
use trogon_std::env::ReadEnv;

use super::{BrokerCore, DispatchSigningError};

impl BrokerCore {
    /// Signs every lease dispatch with `signer` so workers can authenticate it.
    pub fn with_dispatch_signer(mut self, signer: Arc<DispatchSigner>) -> Self {
        self.dispatch_signer = Some(signer);
        self
    }

    /// Adds a fresh signed envelope to a lease dispatch when a signing key is
    /// configured. Called at send time, so a re-sent stored dispatch gets a
    /// new expiry. A dispatch that cannot be signed is refused, never sent
    /// unsigned, because a worker in `warn` mode would accept it.
    pub(crate) fn sign_dispatch(&self, payload: Bytes) -> Result<Bytes, DispatchSignError> {
        let Some(signer) = &self.dispatch_signer else {
            return Ok(payload);
        };
        signer.sign_payload(&payload, OP_LEASE_DISPATCH).map(Bytes::from)
    }

    /// Installs the signer configured in the environment. No key leaves
    /// dispatches unsigned with a warning; a bad key is a startup error.
    pub fn with_dispatch_signer_from_env(self, env: &impl ReadEnv) -> Result<Self, DispatchSigningError> {
        match DispatchSigner::from_env(env) {
            Ok(Some(signer)) => {
                info!(key_id = signer.key_id(), "lease dispatches are signed");
                Ok(self.with_dispatch_signer(Arc::new(signer)))
            }
            Ok(None) => {
                warn!("no dispatch signing key configured; lease dispatches are sent unsigned");
                Ok(self)
            }
            Err(error) => Err(DispatchSigningError::Misconfigured(error)),
        }
    }
}
