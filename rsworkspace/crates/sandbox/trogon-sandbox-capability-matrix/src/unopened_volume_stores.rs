use std::sync::Arc;

use async_trait::async_trait;
use object_store::ObjectStore;
use trogon_sandbox_contract::{CredentialScope, TenantId};
use trogon_sandbox_modal_worker::{VolumeStores, WorkspaceError};

/// Volume stores for a provider that is built only to read its profile and never leases: every open is
/// refused.
pub(crate) struct UnopenedVolumeStores;

#[async_trait]
impl VolumeStores for UnopenedVolumeStores {
    async fn open(&self, _tenant: &TenantId, _scope: &CredentialScope) -> Result<Arc<dyn ObjectStore>, WorkspaceError> {
        Err(WorkspaceError::ScopeUnsupported)
    }
}
