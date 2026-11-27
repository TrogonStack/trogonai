use async_trait::async_trait;
use trogon_sandbox_contract::TenantId;

use super::NativePassthroughPolicy;
use crate::tenant_record::TenantRecordError;

/// The default: no tenant may use passthrough.
pub struct DenyNativePassthrough;

#[async_trait]
impl NativePassthroughPolicy for DenyNativePassthrough {
    async fn allows(&self, _tenant: &TenantId) -> Result<bool, TenantRecordError> {
        Ok(false)
    }
}
