use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::entry::CachedSchema;
use super::errors::SchemaCacheError;
use super::key::{SchemaCacheKey, ServerId};

#[async_trait]
pub trait SchemaCache: Send + Sync {
    async fn get(&self, key: &SchemaCacheKey) -> Result<Option<CachedSchema>, SchemaCacheError>;

    async fn put(&self, key: SchemaCacheKey, entry: CachedSchema) -> Result<(), SchemaCacheError>;

    async fn invalidate(&self, server_id: &ServerId) -> Result<(), SchemaCacheError>;
}

#[derive(Debug, Default)]
pub struct InMemorySchemaCache {
    entries: RwLock<HashMap<SchemaCacheKey, CachedSchema>>,
}

impl InMemorySchemaCache {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SchemaCache for InMemorySchemaCache {
    async fn get(&self, key: &SchemaCacheKey) -> Result<Option<CachedSchema>, SchemaCacheError> {
        Ok(self.entries.read().await.get(key).cloned())
    }

    async fn put(&self, key: SchemaCacheKey, entry: CachedSchema) -> Result<(), SchemaCacheError> {
        self.entries.write().await.insert(key, entry);
        Ok(())
    }

    async fn invalidate(&self, server_id: &ServerId) -> Result<(), SchemaCacheError> {
        self.entries
            .write()
            .await
            .retain(|key, _| key.server_id != *server_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;
    use crate::schema_cache::entry::SchemaSource;
    use crate::schema_cache::key::SchemaHash;

    fn sample_entry(label: &str) -> CachedSchema {
        CachedSchema {
            schema: serde_json::json!({ "label": label }),
            fetched_at: SystemTime::UNIX_EPOCH,
            source: SchemaSource::ToolsListSniff,
        }
    }

    fn key(server_id: &str, byte: u8) -> SchemaCacheKey {
        SchemaCacheKey {
            server_id: ServerId::new(server_id),
            schema_hash: SchemaHash::from_bytes([byte; 32]),
        }
    }

    #[tokio::test]
    async fn get_returns_none_for_missing_key() {
        let cache = InMemorySchemaCache::new();
        let result = cache.get(&key("server-a", 1)).await.expect("get");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn put_then_get_returns_entry() {
        let cache = InMemorySchemaCache::new();
        let cache_key = key("server-a", 1);
        let entry = sample_entry("alpha");

        cache
            .put(cache_key.clone(), entry.clone())
            .await
            .expect("put");
        let got = cache.get(&cache_key).await.expect("get").expect("entry");
        assert_eq!(got, entry);
    }

    #[tokio::test]
    async fn invalidate_removes_all_entries_for_server() {
        let cache = InMemorySchemaCache::new();
        let server_a_key_one = key("server-a", 1);
        let server_a_key_two = key("server-a", 2);
        let server_b_key = key("server-b", 3);

        cache
            .put(server_a_key_one.clone(), sample_entry("a1"))
            .await
            .expect("put a1");
        cache
            .put(server_a_key_two.clone(), sample_entry("a2"))
            .await
            .expect("put a2");
        cache
            .put(server_b_key.clone(), sample_entry("b1"))
            .await
            .expect("put b1");

        cache
            .invalidate(&ServerId::new("server-a"))
            .await
            .expect("invalidate");

        assert!(cache.get(&server_a_key_one).await.expect("get").is_none());
        assert!(cache.get(&server_a_key_two).await.expect("get").is_none());
        assert!(cache.get(&server_b_key).await.expect("get").is_some());
    }
}
