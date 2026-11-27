use std::collections::HashMap;

use async_trait::async_trait;
use trogon_sandbox_contract::TenantId;

use super::{NativePassthroughPolicy, TenantNativePassthroughPolicy};
use crate::tenant_record::TenantRecordError;

/// Per-tenant records held in memory, for the in-memory broker backend.
#[derive(Default)]
pub struct InMemoryNativePassthroughStore {
    policies: tokio::sync::Mutex<HashMap<TenantId, TenantNativePassthroughPolicy>>,
}

impl InMemoryNativePassthroughStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, tenant: &TenantId, policy: TenantNativePassthroughPolicy) {
        self.policies.lock().await.insert(tenant.clone(), policy);
    }
}

#[async_trait]
impl NativePassthroughPolicy for InMemoryNativePassthroughStore {
    async fn allows(&self, tenant: &TenantId) -> Result<bool, TenantRecordError> {
        Ok(self
            .policies
            .lock()
            .await
            .get(tenant)
            .is_some_and(|policy| policy.allowed))
    }
}
