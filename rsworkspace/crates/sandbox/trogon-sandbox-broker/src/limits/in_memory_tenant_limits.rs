use std::collections::HashMap;

use async_trait::async_trait;
use trogon_sandbox_contract::TenantId;

use super::{TenantLimit, TenantLimits};
use crate::tenant_record::TenantRecordError;

#[derive(Default)]
pub struct InMemoryTenantLimits {
    limits: tokio::sync::Mutex<HashMap<TenantId, TenantLimit>>,
}

impl InMemoryTenantLimits {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, tenant: &TenantId, limit: TenantLimit) {
        self.limits.lock().await.insert(tenant.clone(), limit);
    }
}

#[async_trait]
impl TenantLimits for InMemoryTenantLimits {
    async fn for_tenant(&self, tenant: &TenantId) -> Result<Option<TenantLimit>, TenantRecordError> {
        Ok(self.limits.lock().await.get(tenant).cloned())
    }
}
