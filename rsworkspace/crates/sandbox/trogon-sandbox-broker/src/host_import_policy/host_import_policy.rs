use async_trait::async_trait;
use trogon_sandbox_contract::{HostImport, LeaseError, TenantId};

#[async_trait]
pub trait HostImportPolicy: Send + Sync {
    async fn validate(&self, tenant: &TenantId, requested: &[HostImport]) -> Result<(), LeaseError>;
}
