use async_trait::async_trait;
use trogon_sandbox_contract::TenantId;

use super::TenantLimit;
use crate::tenant_record::TenantRecordError;

#[async_trait]
pub trait TenantLimits: Send + Sync {
    async fn for_tenant(&self, tenant: &TenantId) -> Result<Option<TenantLimit>, TenantRecordError>;
}
