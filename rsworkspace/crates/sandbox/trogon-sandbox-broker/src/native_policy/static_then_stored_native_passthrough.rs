use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_contract::TenantId;

use super::{NativePassthroughPolicy, TenantAllowListNativePassthrough};
use crate::tenant_record::TenantRecordError;

/// Static list first, then the stored per-tenant records.
pub struct StaticThenStoredNativePassthrough {
    static_list: TenantAllowListNativePassthrough,
    stored: Arc<dyn NativePassthroughPolicy>,
}

impl StaticThenStoredNativePassthrough {
    pub fn new(static_tenants: impl IntoIterator<Item = TenantId>, stored: Arc<dyn NativePassthroughPolicy>) -> Self {
        Self {
            static_list: TenantAllowListNativePassthrough::new(static_tenants),
            stored,
        }
    }
}

#[async_trait]
impl NativePassthroughPolicy for StaticThenStoredNativePassthrough {
    async fn allows(&self, tenant: &TenantId) -> Result<bool, TenantRecordError> {
        if self.static_list.allows(tenant).await? {
            return Ok(true);
        }
        self.stored.allows(tenant).await
    }
}
