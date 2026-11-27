use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::Mutex;
use trogon_sandbox_contract::{LeaseId, LeaseRecord, TenantId};

use super::{LeaseUsageStore, UsageEntry, UsageStoreError};

#[derive(Default)]
pub struct InMemoryLeaseUsageStore {
    entries: Mutex<HashMap<String, UsageEntry>>,
}

impl InMemoryLeaseUsageStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl LeaseUsageStore for InMemoryLeaseUsageStore {
    async fn modify(
        &self,
        lease: &LeaseRecord,
        change: &(dyn for<'e> Fn(&'e mut UsageEntry) + Send + Sync),
    ) -> Result<(), UsageStoreError> {
        let mut entries = self.entries.lock().await;
        let entry = entries
            .entry(lease.lease_id.as_str().to_owned())
            .or_insert_with(|| UsageEntry::new(lease));
        change(entry);
        Ok(())
    }

    async fn get(&self, tenant: &TenantId, lease_id: &LeaseId) -> Result<Option<UsageEntry>, UsageStoreError> {
        Ok(self
            .entries
            .lock()
            .await
            .get(lease_id.as_str())
            .filter(|entry| &entry.tenant == tenant)
            .cloned())
    }

    async fn tenant_entries(&self, tenant: &TenantId) -> Result<Vec<UsageEntry>, UsageStoreError> {
        Ok(self
            .entries
            .lock()
            .await
            .values()
            .filter(|entry| &entry.tenant == tenant)
            .cloned()
            .collect())
    }
}
