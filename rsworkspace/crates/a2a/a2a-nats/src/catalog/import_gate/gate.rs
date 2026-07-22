use async_trait::async_trait;

use crate::agent_id::A2aAgentId;

use super::error::ImportGateError;
use super::principal::{ImportedAccountName, SpiceDbPrincipal};

#[async_trait]
pub trait ImportGate: Send + Sync {
    async fn permit(
        &self,
        principal: &SpiceDbPrincipal,
        imported_from: &ImportedAccountName,
        agent_id: &A2aAgentId,
    ) -> Result<bool, ImportGateError>;
}
