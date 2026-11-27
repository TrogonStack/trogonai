use std::collections::HashSet;

use async_trait::async_trait;
use trogon_sandbox_contract::TenantId;

use super::NativePassthroughPolicy;
use crate::tenant_record::TenantRecordError;

/// Allows exactly the listed tenants.
pub struct TenantAllowListNativePassthrough {
    tenants: HashSet<TenantId>,
}

impl TenantAllowListNativePassthrough {
    pub fn new(tenants: impl IntoIterator<Item = TenantId>) -> Self {
        Self {
            tenants: tenants.into_iter().collect(),
        }
    }
}

#[async_trait]
impl NativePassthroughPolicy for TenantAllowListNativePassthrough {
    async fn allows(&self, tenant: &TenantId) -> Result<bool, TenantRecordError> {
        Ok(self.tenants.contains(tenant))
    }
}
