use async_trait::async_trait;

use crate::agent_id::A2aAgentId;

use super::error::ImportGateError;
use super::gate::ImportGate;
use super::principal::{ImportedAccountName, SpiceDbPrincipal};

pub struct SpiceDbImportGate {
    #[allow(dead_code)]
    _zed_token_cache: (),
    #[allow(dead_code)]
    _client: (),
}

impl SpiceDbImportGate {
    pub fn new() -> Self {
        Self {
            _zed_token_cache: (),
            _client: (),
        }
    }
}

impl Default for SpiceDbImportGate {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImportGate for SpiceDbImportGate {
    async fn permit(
        &self,
        _principal: &SpiceDbPrincipal,
        _imported_from: &ImportedAccountName,
        _agent_id: &A2aAgentId,
    ) -> Result<bool, ImportGateError> {
        // Authzed SpiceDB BulkCheck hooks land in Phase 1. Until `_client` is wired, federation imports deny
        // rather than succeeding quietly or trapping with `unimplemented!`. Use [`AllowAllImportGate`] for labs.
        let _ = (&self._client, &self._zed_token_cache);
        tracing::debug!("SpiceDbImportGate: rejecting import — Authzed client not configured");
        Ok(false)
    }
}
