use std::collections::HashMap;

use async_trait::async_trait;
use trogon_sandbox_contract::TenantId;

use super::{HostMountAllowList, TenantHostMountPolicy};
use crate::tenant_record::TenantRecordError;

#[derive(Default)]
pub struct InMemoryHostMountAllowList {
    policies: tokio::sync::Mutex<HashMap<TenantId, TenantHostMountPolicy>>,
}

impl InMemoryHostMountAllowList {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, tenant: &TenantId, policy: TenantHostMountPolicy) {
        self.policies.lock().await.insert(tenant.clone(), policy);
    }
}

#[async_trait]
impl HostMountAllowList for InMemoryHostMountAllowList {
    async fn allowed_for(&self, tenant: &TenantId) -> Result<Option<TenantHostMountPolicy>, TenantRecordError> {
        Ok(self.policies.lock().await.get(tenant).cloned())
    }
}
