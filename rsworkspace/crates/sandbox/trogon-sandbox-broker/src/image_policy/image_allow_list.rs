use async_trait::async_trait;
use trogon_sandbox_contract::TenantId;

use super::TenantImagePolicy;
use crate::tenant_record::TenantRecordError;

#[async_trait]
pub trait ImageAllowList: Send + Sync {
    async fn allowed_for(&self, tenant: &TenantId) -> Result<Option<TenantImagePolicy>, TenantRecordError>;
}
