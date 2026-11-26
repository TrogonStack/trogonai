use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::{ApiKeyHash, ApiKeyRecord, ApiKeyStore, ApiKeyStoreError};

/// An [`ApiKeyStore`] held in process memory, for tests and single-process setups.
#[derive(Default)]
pub struct InMemoryApiKeyStore {
    keys: RwLock<HashMap<ApiKeyHash, ApiKeyRecord>>,
}

impl InMemoryApiKeyStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, key_hash: ApiKeyHash, record: ApiKeyRecord) {
        self.keys.write().await.insert(key_hash, record);
    }
}

#[async_trait]
impl ApiKeyStore for InMemoryApiKeyStore {
    async fn lookup(&self, key_hash: &ApiKeyHash) -> Result<Option<ApiKeyRecord>, ApiKeyStoreError> {
        Ok(self.keys.read().await.get(key_hash).cloned())
    }
}
