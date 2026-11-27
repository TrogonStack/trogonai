use super::*;

pub struct InMemoryPrewarmStore {
    entries: RwLock<HashMap<String, PrewarmState>>,
}

impl InMemoryPrewarmStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            entries: RwLock::new(HashMap::new()),
        })
    }

    fn key(tenant: &TenantId, digest: &Digest) -> String {
        prewarm_kv_key(tenant, digest)
    }
}

impl Default for InMemoryPrewarmStore {
    fn default() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl PrewarmStore for InMemoryPrewarmStore {
    async fn get(&self, tenant: &TenantId, digest: &Digest) -> Result<Option<PrewarmState>, PrewarmStoreError> {
        Ok(self.entries.read().await.get(&Self::key(tenant, digest)).cloned())
    }

    async fn put(&self, tenant: &TenantId, digest: &Digest, state: &PrewarmState) -> Result<(), PrewarmStoreError> {
        self.entries
            .write()
            .await
            .insert(Self::key(tenant, digest), state.clone());
        Ok(())
    }

    async fn delete(&self, tenant: &TenantId, digest: &Digest) -> Result<(), PrewarmStoreError> {
        self.entries.write().await.remove(&Self::key(tenant, digest));
        Ok(())
    }

    async fn list_for_tenant(&self, tenant: &TenantId) -> Result<Vec<(Digest, PrewarmState)>, PrewarmStoreError> {
        let mut out = Vec::new();
        for state in self.entries.read().await.values() {
            if state.tenant == *tenant {
                out.push((state.digest.clone(), state.clone()));
            }
        }
        Ok(out)
    }

    async fn list_all(&self) -> Result<Vec<(TenantId, Digest, PrewarmState)>, PrewarmStoreError> {
        let mut out = Vec::new();
        for state in self.entries.read().await.values() {
            out.push((state.tenant.clone(), state.digest.clone(), state.clone()));
        }
        Ok(out)
    }
}
