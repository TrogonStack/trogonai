use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::config::SchemaCacheConfig;
use super::entry::CachedSchema;
use super::errors::SchemaCacheError;
use super::key::{SchemaCacheKey, SchemaHash, ServerId};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ToolIndexKey {
    server_id: ServerId,
    tool_name: String,
}

struct CacheEntry {
    schema: CachedSchema,
    last_access: SystemTime,
}

#[async_trait]
pub trait SchemaCache: Send + Sync {
    async fn get(&self, key: &SchemaCacheKey) -> Result<Option<CachedSchema>, SchemaCacheError>;

    async fn put(
        &self,
        key: SchemaCacheKey,
        entry: CachedSchema,
        tool_name: &str,
    ) -> Result<(), SchemaCacheError>;

    async fn invalidate(&self, server_id: &ServerId) -> Result<(), SchemaCacheError>;

    async fn lookup_tool(&self, server_id: &ServerId, tool_name: &str) -> Result<Option<CachedSchema>, SchemaCacheError>;

    async fn entry_count(&self) -> usize;
}

pub struct InMemorySchemaCache {
    config: SchemaCacheConfig,
    entries: RwLock<HashMap<SchemaCacheKey, CacheEntry>>,
    tool_index: RwLock<HashMap<ToolIndexKey, SchemaHash>>,
    lru: RwLock<VecDeque<SchemaCacheKey>>,
}

impl InMemorySchemaCache {
    pub fn new(config: SchemaCacheConfig) -> Self {
        Self {
            config,
            entries: RwLock::new(HashMap::new()),
            tool_index: RwLock::new(HashMap::new()),
            lru: RwLock::new(VecDeque::new()),
        }
    }

    pub fn config(&self) -> &SchemaCacheConfig {
        &self.config
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    fn is_fresh(&self, entry: &CacheEntry, now: SystemTime) -> bool {
        entry
            .schema
            .fetched_at
            .checked_add(self.config.ttl)
            .is_some_and(|expires| now < expires)
    }

    async fn touch_lru(&self, key: &SchemaCacheKey) {
        let mut lru = self.lru.write().await;
        lru.retain(|existing| existing != key);
        lru.push_back(key.clone());
    }

    async fn remove_key(&self, key: &SchemaCacheKey) {
        self.entries.write().await.remove(key);
        self.tool_index.write().await.retain(|tool_key, hash| {
            !(tool_key.server_id == key.server_id && *hash == key.schema_hash)
        });
        self.lru.write().await.retain(|existing| existing != key);
    }

    async fn evict_if_needed(&self) {
        loop {
            let victim = {
                let entries = self.entries.read().await;
                if entries.len() < self.config.max_entries {
                    return;
                }
                drop(entries);
                let mut lru = self.lru.write().await;
                lru.pop_front()
            };
            let Some(victim) = victim else {
                return;
            };
            self.remove_key(&victim).await;
        }
    }
}

#[async_trait]
impl SchemaCache for InMemorySchemaCache {
    async fn get(&self, key: &SchemaCacheKey) -> Result<Option<CachedSchema>, SchemaCacheError> {
        if !self.config.enabled {
            return Ok(None);
        }
        let now = SystemTime::now();
        let mut entries = self.entries.write().await;
        let Some(entry) = entries.get_mut(key) else {
            return Ok(None);
        };
        if !self.is_fresh(entry, now) {
            drop(entries);
            self.remove_key(key).await;
            return Ok(None);
        }
        entry.last_access = now;
        let cached = entry.schema.clone();
        drop(entries);
        self.touch_lru(key).await;
        Ok(Some(cached))
    }

    async fn put(
        &self,
        key: SchemaCacheKey,
        entry: CachedSchema,
        tool_name: &str,
    ) -> Result<(), SchemaCacheError> {
        if !self.config.enabled {
            return Ok(());
        }
        self.evict_if_needed().await;
        let now = SystemTime::now();
        self.entries.write().await.insert(
            key.clone(),
            CacheEntry {
                schema: entry,
                last_access: now,
            },
        );
        self.tool_index.write().await.insert(
            ToolIndexKey {
                server_id: key.server_id.clone(),
                tool_name: tool_name.to_string(),
            },
            key.schema_hash,
        );
        self.touch_lru(&key).await;
        Ok(())
    }

    async fn invalidate(&self, server_id: &ServerId) -> Result<(), SchemaCacheError> {
        let keys: Vec<_> = self
            .entries
            .read()
            .await
            .keys()
            .filter(|key| key.server_id == *server_id)
            .cloned()
            .collect();
        for key in keys {
            self.remove_key(&key).await;
        }
        self.tool_index
            .write()
            .await
            .retain(|key, _| key.server_id != *server_id);
        Ok(())
    }

    async fn lookup_tool(&self, server_id: &ServerId, tool_name: &str) -> Result<Option<CachedSchema>, SchemaCacheError> {
        let index_key = ToolIndexKey {
            server_id: server_id.clone(),
            tool_name: tool_name.to_string(),
        };
        let schema_hash = {
            let tool_index = self.tool_index.read().await;
            tool_index.get(&index_key).copied()
        };
        let Some(schema_hash) = schema_hash else {
            return Ok(None);
        };
        self.get(&SchemaCacheKey {
            server_id: server_id.clone(),
            schema_hash,
        })
        .await
    }

    async fn entry_count(&self) -> usize {
        self.entries.read().await.len()
    }
}

#[async_trait]
impl SchemaCache for Arc<InMemorySchemaCache> {
    async fn get(&self, key: &SchemaCacheKey) -> Result<Option<CachedSchema>, SchemaCacheError> {
        self.as_ref().get(key).await
    }

    async fn put(
        &self,
        key: SchemaCacheKey,
        entry: CachedSchema,
        tool_name: &str,
    ) -> Result<(), SchemaCacheError> {
        self.as_ref().put(key, entry, tool_name).await
    }

    async fn invalidate(&self, server_id: &ServerId) -> Result<(), SchemaCacheError> {
        self.as_ref().invalidate(server_id).await
    }

    async fn lookup_tool(&self, server_id: &ServerId, tool_name: &str) -> Result<Option<CachedSchema>, SchemaCacheError> {
        self.as_ref().lookup_tool(server_id, tool_name).await
    }

    async fn entry_count(&self) -> usize {
        self.as_ref().entry_count().await
    }
}

pub type SharedSchemaCache = Arc<InMemorySchemaCache>;

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::schema_cache::entry::SchemaSource;

    fn sample_entry(label: &str) -> CachedSchema {
        CachedSchema {
            schema: serde_json::json!({ "label": label }),
            fetched_at: SystemTime::now(),
            source: SchemaSource::ToolsListSniff,
        }
    }

    fn key(server_id: &str, byte: u8) -> SchemaCacheKey {
        SchemaCacheKey {
            server_id: ServerId::new(server_id),
            schema_hash: SchemaHash::from_bytes([byte; 32]),
        }
    }

    fn cache_with_ttl(ttl: Duration) -> InMemorySchemaCache {
        InMemorySchemaCache::new(SchemaCacheConfig {
            ttl,
            ..SchemaCacheConfig::default()
        })
    }

    #[tokio::test]
    async fn get_returns_none_for_missing_key() {
        let cache = InMemorySchemaCache::new(SchemaCacheConfig::default());
        let result = cache.get(&key("server-a", 1)).await.expect("get");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn put_then_get_returns_entry() {
        let cache = InMemorySchemaCache::new(SchemaCacheConfig::default());
        let cache_key = key("server-a", 1);
        let entry = sample_entry("alpha");

        cache
            .put(cache_key.clone(), entry.clone(), "alpha")
            .await
            .expect("put");
        let got = cache.get(&cache_key).await.expect("get").expect("entry");
        assert_eq!(got, entry);
    }

    #[tokio::test]
    async fn invalidate_removes_all_entries_for_server() {
        let cache = InMemorySchemaCache::new(SchemaCacheConfig::default());
        let server_a_key_one = key("server-a", 1);
        let server_a_key_two = key("server-a", 2);
        let server_b_key = key("server-b", 3);

        cache
            .put(server_a_key_one.clone(), sample_entry("a1"), "a1")
            .await
            .expect("put a1");
        cache
            .put(server_a_key_two.clone(), sample_entry("a2"), "a2")
            .await
            .expect("put a2");
        cache
            .put(server_b_key.clone(), sample_entry("b1"), "b1")
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

    #[tokio::test]
    async fn expired_entry_is_evicted_on_get() {
        let cache = cache_with_ttl(Duration::from_secs(1));
        let cache_key = key("server-a", 9);
        let stale_at = SystemTime::now()
            .checked_sub(Duration::from_secs(5))
            .expect("clock");
        cache
            .put(
                cache_key.clone(),
                CachedSchema {
                    schema: serde_json::json!({ "label": "stale" }),
                    fetched_at: stale_at,
                    source: SchemaSource::ToolsListSniff,
                },
                "stale",
            )
            .await
            .expect("put");
        assert!(cache.get(&cache_key).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn fresh_entry_survives_until_ttl() {
        let cache = cache_with_ttl(Duration::from_secs(60));
        let cache_key = key("server-a", 4);
        cache
            .put(
                cache_key.clone(),
                CachedSchema {
                    schema: serde_json::json!({ "label": "fresh" }),
                    fetched_at: SystemTime::now(),
                    source: SchemaSource::ToolsListSniff,
                },
                "fresh",
            )
            .await
            .expect("put");
        assert!(cache.get(&cache_key).await.expect("get").is_some());
    }

    #[tokio::test]
    async fn lru_evicts_least_recently_used_when_cap_exceeded() {
        let cache = InMemorySchemaCache::new(SchemaCacheConfig {
            max_entries: 2,
            ..SchemaCacheConfig::default()
        });
        let first = key("server-a", 1);
        let second = key("server-a", 2);
        let third = key("server-a", 3);
        cache
            .put(first.clone(), sample_entry("first"), "first")
            .await
            .expect("put first");
        cache
            .put(second.clone(), sample_entry("second"), "second")
            .await
            .expect("put second");
        cache.get(&first).await.expect("touch first");
        cache
            .put(third.clone(), sample_entry("third"), "third")
            .await
            .expect("put third");
        assert!(cache.get(&second).await.expect("get second").is_none());
        assert!(cache.get(&first).await.expect("get first").is_some());
        assert!(cache.get(&third).await.expect("get third").is_some());
    }

    #[tokio::test]
    async fn schema_cache_key_equality_is_stable() {
        let left = key("server-a", 7);
        let right = key("server-a", 7);
        assert_eq!(left, right);
        assert_ne!(left, key("server-b", 7));
    }
}
