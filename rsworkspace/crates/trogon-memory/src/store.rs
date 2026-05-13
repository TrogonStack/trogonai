use async_nats::jetstream::kv;
use bytes::Bytes;
use std::future::Future;

use crate::types::{DreamerError, EntityMemory};

// ── Key helpers ───────────────────────────────────────────────────────────────

/// Build the KV key for an entity's memory.
///
/// Pattern: `{actor_type}.{sanitized_actor_key}` where sanitization replaces
/// `/` and spaces with `_` so the key is safe for NATS KV.
pub fn memory_key(actor_type: &str, actor_key: &str) -> String {
    let sanitized = actor_key
        .chars()
        .map(|c| match c {
            '/' | ' ' => '_',
            c => c,
        })
        .collect::<String>();
    format!("{actor_type}.{sanitized}")
}

// ── MemoryStore trait ─────────────────────────────────────────────────────────

pub trait MemoryStore: Send + Sync + Clone + 'static {
    type PutError: std::error::Error + Send + Sync + 'static;
    type GetError: std::error::Error + Send + Sync + 'static;
    type DeleteError: std::error::Error + Send + Sync + 'static;

    fn put(
        &self,
        key: &str,
        value: Bytes,
    ) -> impl Future<Output = Result<u64, Self::PutError>> + Send;

    fn get(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<Bytes>, Self::GetError>> + Send;

    fn delete(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<(), Self::DeleteError>> + Send;
}

// ── Production implementation ─────────────────────────────────────────────────

impl MemoryStore for kv::Store {
    type PutError = kv::PutError;
    type GetError = kv::EntryError;
    type DeleteError = kv::DeleteError;

    async fn put(&self, key: &str, value: Bytes) -> Result<u64, Self::PutError> {
        kv::Store::put(self, key, value).await
    }

    async fn get(&self, key: &str) -> Result<Option<Bytes>, Self::GetError> {
        kv::Store::get(self, key).await
    }

    async fn delete(&self, key: &str) -> Result<(), Self::DeleteError> {
        kv::Store::delete(self, key).await
    }
}

// ── MemoryClient ──────────────────────────────────────────────────────────────

/// Read/write interface for entity memories. Handles serialization.
///
/// Can be used by other crates (e.g. `trogon-acp-runner`) to load a memory
/// blob and inject it as context at session start.
#[derive(Clone)]
pub struct MemoryClient<S: MemoryStore> {
    store: S,
}

impl<S: MemoryStore> MemoryClient<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// Load the current memory for an entity, or `None` if none exists yet.
    pub async fn get(
        &self,
        actor_type: &str,
        actor_key: &str,
    ) -> Result<Option<EntityMemory>, DreamerError> {
        let key = memory_key(actor_type, actor_key);
        match self.store.get(&key).await {
            Ok(Some(bytes)) => serde_json::from_slice::<EntityMemory>(&bytes)
                .map(Some)
                .map_err(|e| DreamerError::Store(e.to_string())),
            Ok(None) => Ok(None),
            Err(e) => Err(DreamerError::Store(e.to_string())),
        }
    }

    /// Persist an updated memory for an entity.
    pub async fn put(
        &self,
        actor_type: &str,
        actor_key: &str,
        memory: &EntityMemory,
    ) -> Result<(), DreamerError> {
        let key = memory_key(actor_type, actor_key);
        let bytes = serde_json::to_vec(memory)
            .map(Bytes::from)
            .map_err(|e| DreamerError::Store(e.to_string()))?;
        self.store
            .put(&key, bytes)
            .await
            .map(|_| ())
            .map_err(|e| DreamerError::Store(e.to_string()))
    }

    /// Delete all memory for an entity (e.g. for privacy or reset).
    pub async fn delete(
        &self,
        actor_type: &str,
        actor_key: &str,
    ) -> Result<(), DreamerError> {
        let key = memory_key(actor_type, actor_key);
        self.store
            .delete(&key)
            .await
            .map_err(|e| DreamerError::Store(e.to_string()))
    }

    /// Format the accumulated memory as a system prompt prefix.
    ///
    /// Returns `None` when the entity has no memory yet (caller can skip injection).
    pub fn format_for_prompt(memory: &EntityMemory) -> Option<String> {
        if memory.facts.is_empty() {
            return None;
        }

        let mut out = String::from(
            "<entity-memory>\nThe following facts were learned in previous sessions:\n\n",
        );
        for fact in &memory.facts {
            out.push_str(&format!(
                "- [{}] {}\n",
                fact.category, fact.content
            ));
        }
        out.push_str("</entity-memory>");
        Some(out)
    }
}

// ── Test / mock implementation ────────────────────────────────────────────────

#[cfg(any(test, feature = "test-support"))]
pub mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    pub struct MockMemoryStore {
        data: Arc<Mutex<HashMap<String, Bytes>>>,
    }

    impl MockMemoryStore {
        pub fn new() -> Self {
            Self::default()
        }
    }

    impl MemoryStore for MockMemoryStore {
        type PutError = std::convert::Infallible;
        type GetError = std::convert::Infallible;
        type DeleteError = std::convert::Infallible;

        async fn put(&self, key: &str, value: Bytes) -> Result<u64, Self::PutError> {
            self.data.lock().unwrap().insert(key.to_string(), value);
            Ok(0)
        }

        async fn get(&self, key: &str) -> Result<Option<Bytes>, Self::GetError> {
            Ok(self.data.lock().unwrap().get(key).cloned())
        }

        async fn delete(&self, key: &str) -> Result<(), Self::DeleteError> {
            self.data.lock().unwrap().remove(key);
            Ok(())
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{MemoryFact, RawFact};
    use mock::MockMemoryStore;

    fn make_client() -> MemoryClient<MockMemoryStore> {
        MemoryClient::new(MockMemoryStore::new())
    }

    #[tokio::test]
    async fn get_returns_none_when_no_memory_stored() {
        let client = make_client();
        let result = client.get("pr", "owner/repo/1").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn put_and_get_round_trips() {
        let client = make_client();
        let mut memory = EntityMemory::default();
        memory.merge(
            vec![RawFact { category: "fact".into(), content: "uses Rust".into(), confidence: 0.95 }],
            "sess-1",
        );
        client.put("pr", "owner/repo/1", &memory).await.unwrap();
        let loaded = client.get("pr", "owner/repo/1").await.unwrap().unwrap();
        assert_eq!(loaded.facts.len(), 1);
        assert_eq!(loaded.facts[0].content, "uses Rust");
    }

    #[tokio::test]
    async fn different_entities_are_isolated() {
        let client = make_client();
        let mut memory_a = EntityMemory::default();
        memory_a.merge(
            vec![RawFact { category: "fact".into(), content: "entity A".into(), confidence: 1.0 }],
            "s1",
        );
        client.put("pr", "repo/1", &memory_a).await.unwrap();

        let result = client.get("pr", "repo/2").await.unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn memory_key_sanitizes_slashes() {
        assert_eq!(memory_key("pr", "owner/repo/456"), "pr.owner_repo_456");
    }

    #[test]
    fn memory_key_sanitizes_spaces() {
        assert_eq!(memory_key("agent", "my agent"), "agent.my_agent");
    }

    #[tokio::test]
    async fn get_returns_store_error_when_bytes_are_corrupt() {
        // Store that returns bytes that cannot be deserialized as EntityMemory.
        let store = MockMemoryStore::new();
        store
            .put("pr.some_key", bytes::Bytes::from(b"not-valid-json".to_vec()))
            .await
            .unwrap();

        let client = MemoryClient::new(store);
        // get() should return Err(DreamerError::Store(...)), not panic or Ok(None)
        let err = client.get("pr", "some/key").await.unwrap_err();
        assert!(matches!(err, DreamerError::Store(_)));
    }

    #[test]
    fn format_for_prompt_returns_none_when_no_facts() {
        let memory = EntityMemory::default();
        assert!(MemoryClient::<MockMemoryStore>::format_for_prompt(&memory).is_none());
    }

    #[test]
    fn format_for_prompt_includes_all_facts() {
        let mut memory = EntityMemory::default();
        memory.facts.push(MemoryFact {
            category: "preference".into(),
            content: "prefers dark mode".into(),
            confidence: 0.9,
            source_session: "s1".into(),
            timestamp: 0,
        });
        memory.facts.push(MemoryFact {
            category: "goal".into(),
            content: "ship by Friday".into(),
            confidence: 0.8,
            source_session: "s1".into(),
            timestamp: 0,
        });
        let prompt = MemoryClient::<MockMemoryStore>::format_for_prompt(&memory).unwrap();
        assert!(prompt.contains("<entity-memory>"));
        assert!(prompt.contains("prefers dark mode"));
        assert!(prompt.contains("ship by Friday"));
        assert!(prompt.contains("</entity-memory>"));
    }
}
