use async_trait::async_trait;
use trogon_sandbox_contract::{NativeSnapshotRef, TenantId};

#[async_trait]
pub trait NativeSnapshotStore: Send + Sync {
    async fn record(&self, tenant: &TenantId, snapshot: &NativeSnapshotRef) -> anyhow::Result<()>;
    /// Whether `tenant` recorded exactly this provider and id.
    async fn owns(&self, tenant: &TenantId, snapshot: &NativeSnapshotRef) -> anyhow::Result<bool>;

    async fn begin_delete(
        &self,
        _tenant: &TenantId,
        _snapshot: &NativeSnapshotRef,
    ) -> anyhow::Result<Option<NativeSnapshotRef>> {
        anyhow::bail!("native snapshot store cannot persist deletion intent")
    }

    async fn finish_delete(&self, _tenant: &TenantId, _snapshot: &NativeSnapshotRef) -> anyhow::Result<()> {
        anyhow::bail!("native snapshot store cannot persist deletion completion")
    }
}
