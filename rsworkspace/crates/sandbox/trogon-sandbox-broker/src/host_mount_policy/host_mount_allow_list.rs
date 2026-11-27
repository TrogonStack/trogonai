use async_trait::async_trait;
use trogon_sandbox_contract::TenantId;

use super::TenantHostMountPolicy;
use crate::tenant_record::TenantRecordError;

#[async_trait]
pub trait HostMountAllowList: Send + Sync {
    async fn allowed_for(&self, tenant: &TenantId) -> Result<Option<TenantHostMountPolicy>, TenantRecordError>;
}
