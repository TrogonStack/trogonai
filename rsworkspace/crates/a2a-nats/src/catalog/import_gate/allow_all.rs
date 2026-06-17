use async_trait::async_trait;

use crate::agent_id::A2aAgentId;

use super::error::ImportGateError;
use super::gate::ImportGate;
use super::principal::{ImportedAccountName, SpiceDbPrincipal};

pub struct AllowAllImportGate;

#[async_trait]
impl ImportGate for AllowAllImportGate {
    async fn permit(
        &self,
        _principal: &SpiceDbPrincipal,
        _imported_from: &ImportedAccountName,
        _agent_id: &A2aAgentId,
    ) -> Result<bool, ImportGateError> {
        Ok(true)
    }
}
