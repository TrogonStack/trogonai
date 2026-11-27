use super::*;

pub struct NatsKvPrewarmStore {
    pub kv: kv::Store,
}

impl NatsKvPrewarmStore {
    pub async fn new(jetstream: jetstream::Context) -> Result<Self, PrewarmStoreError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: PREWARM_KV_BUCKET.to_string(),
                history: 1,
                max_age: PREWARM_KV_TTL,
                ..Default::default()
            })
            .await
            .map_err(PrewarmStoreError::backend)?;
        Ok(Self { kv })
    }
}

impl NatsKvPrewarmStore {
    async fn read(&self, key: &str) -> Result<Option<PrewarmState>, PrewarmStoreError> {
        match self.kv.get(key).await.map_err(PrewarmStoreError::backend)? {
            None => Ok(None),
            Some(value) => Ok(Some(serde_json::from_slice(&value)?)),
        }
    }

    /// Legacy key of `(tenant, digest)` when it differs from the current key
    /// and holds this tenant's record. Tenants such as `a.b` and `a_b` share
    /// a legacy key, so the record's own tenant decides who it belongs to.
    async fn owned_legacy(
        &self,
        tenant: &TenantId,
        digest: &Digest,
    ) -> Result<Option<(String, PrewarmState)>, PrewarmStoreError> {
        let legacy = legacy_prewarm_kv_key(tenant, digest);
        if legacy == prewarm_kv_key(tenant, digest) {
            return Ok(None);
        }
        Ok(self
            .read(&legacy)
            .await?
            .filter(|state| state.tenant == *tenant && state.digest == *digest)
            .map(|state| (legacy, state)))
    }
}

#[async_trait]
impl PrewarmStore for NatsKvPrewarmStore {
    async fn get(&self, tenant: &TenantId, digest: &Digest) -> Result<Option<PrewarmState>, PrewarmStoreError> {
        if let Some(state) = self.read(&prewarm_kv_key(tenant, digest)).await? {
            return Ok(Some(state));
        }
        Ok(self.owned_legacy(tenant, digest).await?.map(|(_, state)| state))
    }

    async fn put(&self, tenant: &TenantId, digest: &Digest, state: &PrewarmState) -> Result<(), PrewarmStoreError> {
        let key = prewarm_kv_key(tenant, digest);
        let payload = Bytes::from(serde_json::to_vec(state)?);
        self.kv.put(key, payload).await.map_err(PrewarmStoreError::backend)?;
        if let Some((legacy, _)) = self.owned_legacy(tenant, digest).await? {
            self.kv.delete(&legacy).await.map_err(PrewarmStoreError::backend)?;
        }
        Ok(())
    }

    async fn delete(&self, tenant: &TenantId, digest: &Digest) -> Result<(), PrewarmStoreError> {
        let key = prewarm_kv_key(tenant, digest);
        self.kv.delete(&key).await.map_err(PrewarmStoreError::backend)?;
        if let Some((legacy, _)) = self.owned_legacy(tenant, digest).await? {
            self.kv.delete(&legacy).await.map_err(PrewarmStoreError::backend)?;
        }
        Ok(())
    }

    async fn list_for_tenant(&self, tenant: &TenantId) -> Result<Vec<(Digest, PrewarmState)>, PrewarmStoreError> {
        let prefix = tenant_scoped_kv_prefix(tenant);
        let legacy_prefix = legacy_tenant_scoped_kv_prefix(tenant);
        let mut current = BTreeMap::new();
        let mut legacy = BTreeMap::new();
        let mut keys = self.kv.keys().await.map_err(PrewarmStoreError::backend)?;
        while let Some(key) = keys.next().await {
            let key = key.map_err(PrewarmStoreError::backend)?;
            let is_current = key.starts_with(&prefix);
            if !is_current && !key.starts_with(&legacy_prefix) {
                continue;
            }
            let Some(state) = self.read(&key).await? else {
                continue;
            };
            if state.tenant != *tenant {
                continue;
            }
            let found = if is_current { &mut current } else { &mut legacy };
            found.insert(state.digest.as_str().to_string(), state);
        }
        for (digest, state) in legacy {
            current.entry(digest).or_insert(state);
        }
        Ok(current
            .into_values()
            .map(|state| (state.digest.clone(), state))
            .collect())
    }

    async fn list_all(&self) -> Result<Vec<(TenantId, Digest, PrewarmState)>, PrewarmStoreError> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut keys = self.kv.keys().await.map_err(PrewarmStoreError::backend)?;
        while let Some(key) = keys.next().await {
            let key = key.map_err(PrewarmStoreError::backend)?;
            let Some(state) = self.read(&key).await? else {
                continue;
            };
            if seen.insert((state.tenant.as_str().to_string(), state.digest.as_str().to_string())) {
                out.push((state.tenant.clone(), state.digest.clone(), state));
            }
        }
        Ok(out)
    }
}
