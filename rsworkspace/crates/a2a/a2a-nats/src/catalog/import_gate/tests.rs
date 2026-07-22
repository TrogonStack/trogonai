use async_trait::async_trait;

use crate::agent_id::A2aAgentId;

use super::allow_all::AllowAllImportGate;
use super::error::ImportGateError;
use super::gate::ImportGate;
use super::principal::{ImportedAccountName, SpiceDbPrincipal};

struct DenyAllImportGate;

#[async_trait]
impl ImportGate for DenyAllImportGate {
    async fn permit(
        &self,
        _principal: &SpiceDbPrincipal,
        _imported_from: &ImportedAccountName,
        _agent_id: &A2aAgentId,
    ) -> Result<bool, ImportGateError> {
        Ok(false)
    }
}

#[tokio::test]
async fn allow_all_permits_everything() {
    let gate = AllowAllImportGate;
    let p = SpiceDbPrincipal::new("user:alice");
    let imported = ImportedAccountName::new("peer-acct");
    let aid = A2aAgentId::new("bot").unwrap();
    assert!(
        gate.permit(&p, &imported, &aid)
            .await
            .expect("AllowAll gate must succeed")
    );
}

#[tokio::test]
async fn deny_all_denies_when_import_source_claimed() {
    let gate = DenyAllImportGate;
    let p = SpiceDbPrincipal::new("user:alice");
    let imported = ImportedAccountName::new("peer-acct");
    let aid = A2aAgentId::new("bot").unwrap();
    assert!(!gate.permit(&p, &imported, &aid).await.expect("must not error"));
}
