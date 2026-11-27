use super::*;

#[async_trait]
pub trait PrewarmStore: Send + Sync {
    async fn get(&self, tenant: &TenantId, digest: &Digest) -> Result<Option<PrewarmState>, PrewarmStoreError>;
    async fn put(&self, tenant: &TenantId, digest: &Digest, state: &PrewarmState) -> Result<(), PrewarmStoreError>;
    async fn delete(&self, tenant: &TenantId, digest: &Digest) -> Result<(), PrewarmStoreError>;
    async fn list_for_tenant(&self, tenant: &TenantId) -> Result<Vec<(Digest, PrewarmState)>, PrewarmStoreError>;
    async fn list_all(&self) -> Result<Vec<(TenantId, Digest, PrewarmState)>, PrewarmStoreError>;
}
