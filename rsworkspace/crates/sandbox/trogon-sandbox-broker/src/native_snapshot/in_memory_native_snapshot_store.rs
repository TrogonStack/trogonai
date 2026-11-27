use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::RwLock;
use trogon_sandbox_contract::{NativeSnapshotRef, TenantId};
use trogon_sandbox_protocol::tenant_scoped_kv_key;

use super::NativeSnapshotStore;
use super::owned_snapshot::{OwnedSnapshot, SnapshotState, entry_name};

#[derive(Default)]
pub struct InMemoryNativeSnapshotStore {
    owned: RwLock<HashMap<String, OwnedSnapshot>>,
}

impl InMemoryNativeSnapshotStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl NativeSnapshotStore for InMemoryNativeSnapshotStore {
    async fn record(&self, tenant: &TenantId, snapshot: &NativeSnapshotRef) -> anyhow::Result<()> {
        let record = OwnedSnapshot::live(tenant, snapshot);
        let key = tenant_scoped_kv_key(tenant, &entry_name(snapshot));
        let mut owned = self.owned.write().await;
        if let Some(existing) = owned.get_mut(&key) {
            anyhow::ensure!(
                existing.state == SnapshotState::Live,
                "native snapshot identity is already retired"
            );
            existing.reference = snapshot.clone();
        } else {
            owned.insert(key, record);
        }
        Ok(())
    }

    async fn owns(&self, tenant: &TenantId, snapshot: &NativeSnapshotRef) -> anyhow::Result<bool> {
        let key = tenant_scoped_kv_key(tenant, &entry_name(snapshot));
        Ok(self
            .owned
            .read()
            .await
            .get(&key)
            .is_some_and(|record| record.state == SnapshotState::Live))
    }

    async fn begin_delete(
        &self,
        tenant: &TenantId,
        snapshot: &NativeSnapshotRef,
    ) -> anyhow::Result<Option<NativeSnapshotRef>> {
        let key = tenant_scoped_kv_key(tenant, &entry_name(snapshot));
        let mut owned = self.owned.write().await;
        let Some(record) = owned.get_mut(&key) else {
            return Ok(None);
        };
        if record.state == SnapshotState::Deleted {
            return Ok(None);
        }
        record.state = SnapshotState::Deleting;
        Ok(Some(record.reference.clone()))
    }

    async fn finish_delete(&self, tenant: &TenantId, snapshot: &NativeSnapshotRef) -> anyhow::Result<()> {
        let key = tenant_scoped_kv_key(tenant, &entry_name(snapshot));
        let mut owned = self.owned.write().await;
        let record = owned
            .get_mut(&key)
            .ok_or_else(|| anyhow::anyhow!("native snapshot deletion intent is missing"))?;
        anyhow::ensure!(
            record.state != SnapshotState::Live,
            "native snapshot deletion intent is missing"
        );
        record.state = SnapshotState::Deleted;
        Ok(())
    }
}
