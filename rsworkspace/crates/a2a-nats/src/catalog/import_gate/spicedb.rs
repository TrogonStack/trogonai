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
        // TODO(spicedb-client): Phase 1 gateway SpiceDB integration (see `A2A_TODO.md`).
        unimplemented!("wired when gateway SpiceDB client lands");
    }
}
