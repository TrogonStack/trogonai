use std::collections::HashMap;

use async_trait::async_trait;
use trogon_sandbox_contract::TenantId;

use super::{ImageAllowList, TenantImagePolicy};
use crate::tenant_record::TenantRecordError;

#[derive(Default)]
pub struct InMemoryImageAllowList {
    policies: tokio::sync::Mutex<HashMap<TenantId, TenantImagePolicy>>,
}

impl InMemoryImageAllowList {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, tenant: &TenantId, policy: TenantImagePolicy) {
        self.policies.lock().await.insert(tenant.clone(), policy);
    }
}

#[async_trait]
impl ImageAllowList for InMemoryImageAllowList {
    async fn allowed_for(&self, tenant: &TenantId) -> Result<Option<TenantImagePolicy>, TenantRecordError> {
        Ok(self.policies.lock().await.get(tenant).cloned())
    }
}
