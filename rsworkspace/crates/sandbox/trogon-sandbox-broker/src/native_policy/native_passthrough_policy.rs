use async_trait::async_trait;
use trogon_sandbox_contract::TenantId;

use crate::tenant_record::TenantRecordError;

#[async_trait]
pub trait NativePassthroughPolicy: Send + Sync {
    /// Whether `tenant` may send `native` options and read native handles.
    async fn allows(&self, tenant: &TenantId) -> Result<bool, TenantRecordError>;
}
