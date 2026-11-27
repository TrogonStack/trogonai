use std::sync::Arc;

use async_trait::async_trait;
use object_store::ObjectStore;
use trogon_sandbox_contract::{CredentialScope, TenantId};

use crate::WorkspaceError;

/// Opens the object store a volume scope names, with whatever credentials that takes.
#[async_trait]
pub trait VolumeStores: Send + Sync {
    async fn open(&self, tenant: &TenantId, scope: &CredentialScope) -> Result<Arc<dyn ObjectStore>, WorkspaceError>;
}
