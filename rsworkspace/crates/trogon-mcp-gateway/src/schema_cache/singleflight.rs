use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use tokio::sync::Mutex;

use super::entry::CachedSchema;
use super::errors::SchemaCacheError;
use super::key::SchemaCacheKey;
use super::store::{InMemorySchemaCache, SchemaCache};

pub struct SchemaSingleflight {
    in_flight: Mutex<HashMap<SchemaCacheKey, Arc<Mutex<()>>>>,
}

impl Default for SchemaSingleflight {
    fn default() -> Self {
        Self {
            in_flight: Mutex::new(HashMap::new()),
        }
    }
}

impl SchemaSingleflight {
    pub async fn fetch_or_wait<F, Fut>(
        &self,
        key: SchemaCacheKey,
        cache: &InMemorySchemaCache,
        fetch: F,
    ) -> Result<Option<CachedSchema>, SchemaCacheError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<CachedSchema, SchemaCacheError>> + Send,
    {
        if let Some(hit) = cache.get(&key).await? {
            return Ok(Some(hit));
        }

        let gate = {
            let mut map = self.in_flight.lock().await;
            map.entry(key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };

        let _leader = gate.lock().await;
        if let Some(hit) = cache.get(&key).await? {
            self.in_flight.lock().await.remove(&key);
            return Ok(Some(hit));
        }

        let fetched = fetch().await?;
        cache
            .put(key.clone(), fetched.clone(), "singleflight")
            .await?;
        self.in_flight.lock().await.remove(&key);
        Ok(Some(fetched))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::SystemTime;

    use super::*;
    use crate::schema_cache::config::SchemaCacheConfig;
    use crate::schema_cache::entry::SchemaSource;
    use crate::schema_cache::hash::hash_schema;
    use crate::schema_cache::key::{SchemaHash, ServerId};

    #[tokio::test]
    async fn concurrent_misses_deduplicate_fetch() {
        let cache = Arc::new(InMemorySchemaCache::new(SchemaCacheConfig::default()));
        let singleflight = Arc::new(SchemaSingleflight::default());
        let fetch_count = Arc::new(AtomicUsize::new(0));
        let schema = serde_json::json!({"type": "object"});
        let key = SchemaCacheKey {
            server_id: ServerId::new("server-a"),
            schema_hash: hash_schema(&schema),
        };

        let mut handles = Vec::new();
        for _ in 0..4 {
            let cache = cache.clone();
            let singleflight = singleflight.clone();
            let fetch_count = fetch_count.clone();
            let key = key.clone();
            let schema = schema.clone();
            handles.push(tokio::spawn(async move {
                singleflight
                    .fetch_or_wait(key, &cache, || {
                        let fetch_count = fetch_count.clone();
                        let schema = schema.clone();
                        async move {
                            fetch_count.fetch_add(1, Ordering::SeqCst);
                            Ok(CachedSchema {
                                schema,
                                fetched_at: SystemTime::now(),
                                source: SchemaSource::ToolsListSniff,
                            })
                        }
                    })
                    .await
            }));
        }

        for handle in handles {
            assert!(handle.await.expect("join").expect("fetch").is_some());
        }
        assert_eq!(fetch_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn unrelated_keys_fetch_in_parallel() {
        let cache = Arc::new(InMemorySchemaCache::new(SchemaCacheConfig::default()));
        let singleflight = Arc::new(SchemaSingleflight::default());
        let fetch_count = Arc::new(AtomicUsize::new(0));

        let key_a = SchemaCacheKey {
            server_id: ServerId::new("server-a"),
            schema_hash: SchemaHash::from_bytes([1; 32]),
        };
        let key_b = SchemaCacheKey {
            server_id: ServerId::new("server-b"),
            schema_hash: SchemaHash::from_bytes([2; 32]),
        };

        let sf = singleflight.clone();
        let c = cache.clone();
        let fc = fetch_count.clone();
        let h1 = tokio::spawn(async move {
            sf.fetch_or_wait(key_a, &c, || {
                let fc = fc.clone();
                async move {
                    fc.fetch_add(1, Ordering::SeqCst);
                    Ok(CachedSchema {
                        schema: serde_json::json!({"a": true}),
                        fetched_at: SystemTime::now(),
                        source: SchemaSource::ToolsListSniff,
                    })
                }
            })
            .await
        });
        let sf = singleflight.clone();
        let c = cache.clone();
        let fc = fetch_count.clone();
        let h2 = tokio::spawn(async move {
            sf.fetch_or_wait(key_b, &c, || {
                let fc = fc.clone();
                async move {
                    fc.fetch_add(1, Ordering::SeqCst);
                    Ok(CachedSchema {
                        schema: serde_json::json!({"b": true}),
                        fetched_at: SystemTime::now(),
                        source: SchemaSource::ToolsListSniff,
                    })
                }
            })
            .await
        });

        h1.await.expect("join1").expect("fetch1");
        h2.await.expect("join2").expect("fetch2");
        assert_eq!(fetch_count.load(Ordering::SeqCst), 2);
    }
}
