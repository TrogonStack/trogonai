use trogon_transcript::TranscriptEntry;

use crate::{
    provider::MemoryProvider,
    store::MemoryClient,
    store::MemoryStore,
    types::{DreamerError, EntityMemory},
};

/// Extracts memory facts from a session transcript and persists them.
///
/// `Dreamer<P, S>` is generic over the LLM provider (`P`) and the KV store (`S`)
/// so it can be fully unit-tested with mocks.
pub struct Dreamer<P: MemoryProvider, S: MemoryStore> {
    provider: P,
    client: MemoryClient<S>,
}

impl<P: MemoryProvider, S: MemoryStore> Dreamer<P, S> {
    pub fn new(provider: P, store: S) -> Self {
        Self { provider, client: MemoryClient::new(store) }
    }

    /// Process a completed session:
    /// 1. Load existing memory for the entity.
    /// 2. Extract new facts from the transcript using the LLM.
    /// 3. Merge facts into the entity's memory and persist.
    ///
    /// Returns the updated `EntityMemory`. If the transcript yields no new facts,
    /// the existing memory is returned unchanged (not persisted).
    pub async fn process_session(
        &self,
        actor_type: &str,
        actor_key: &str,
        session_id: &str,
        transcript: &[TranscriptEntry],
    ) -> Result<EntityMemory, DreamerError> {
        let mut memory = self
            .client
            .get(actor_type, actor_key)
            .await?
            .unwrap_or_default();

        let existing = if memory.facts.is_empty() { None } else { Some(&memory) };

        let new_facts = self
            .provider
            .extract_facts(transcript, existing)
            .await?;

        if new_facts.is_empty() {
            return Ok(memory);
        }

        memory.merge(new_facts, session_id);
        self.client.put(actor_type, actor_key, &memory).await?;

        Ok(memory)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::store::mock::MockMemoryStore;
    use crate::types::{RawFact, now_ms};
    use trogon_transcript::entry::Role;

    // ── MockMemoryProvider ────────────────────────────────────────────────────

    #[derive(Clone)]
    struct MockMemoryProvider {
        facts: Arc<Mutex<Vec<RawFact>>>,
        error: Arc<Mutex<Option<String>>>,
    }

    impl MockMemoryProvider {
        fn returning(facts: Vec<RawFact>) -> Self {
            Self {
                facts: Arc::new(Mutex::new(facts)),
                error: Arc::new(Mutex::new(None)),
            }
        }

        fn failing(msg: &str) -> Self {
            Self {
                facts: Arc::new(Mutex::new(vec![])),
                error: Arc::new(Mutex::new(Some(msg.to_string()))),
            }
        }
    }

    impl MemoryProvider for MockMemoryProvider {
        fn extract_facts<'a>(
            &'a self,
            _transcript: &'a [TranscriptEntry],
            _existing: Option<&'a EntityMemory>,
        ) -> impl Future<Output = Result<Vec<RawFact>, DreamerError>> + Send + 'a {
            let facts = self.facts.lock().unwrap().clone();
            let err = self.error.lock().unwrap().clone();
            async move {
                if let Some(e) = err {
                    Err(DreamerError::Llm(e))
                } else {
                    Ok(facts)
                }
            }
        }
    }

    fn sample_transcript() -> Vec<TranscriptEntry> {
        vec![TranscriptEntry::Message {
            role: Role::User,
            content: "I only use tabs, not spaces.".into(),
            timestamp: now_ms(),
            tokens: None,
        }]
    }

    #[tokio::test]
    async fn process_session_stores_new_facts() {
        let provider = MockMemoryProvider::returning(vec![RawFact {
            category: "preference".into(),
            content: "uses tabs".into(),
            confidence: 0.9,
        }]);
        let store = MockMemoryStore::new();
        let dreamer = Dreamer::new(provider, store.clone());

        let result = dreamer
            .process_session("agent", "proj/123", "sess-1", &sample_transcript())
            .await
            .unwrap();

        assert_eq!(result.facts.len(), 1);
        assert_eq!(result.facts[0].content, "uses tabs");
        assert_eq!(result.facts[0].source_session, "sess-1");

        // Verify it was persisted
        let client = MemoryClient::new(store);
        let loaded = client.get("agent", "proj/123").await.unwrap().unwrap();
        assert_eq!(loaded.facts.len(), 1);
    }

    #[tokio::test]
    async fn process_session_accumulates_facts_across_sessions() {
        let store = MockMemoryStore::new();

        // First session
        let dreamer = Dreamer::new(
            MockMemoryProvider::returning(vec![RawFact {
                category: "fact".into(),
                content: "first fact".into(),
                confidence: 1.0,
            }]),
            store.clone(),
        );
        dreamer
            .process_session("pr", "repo/1", "sess-1", &sample_transcript())
            .await
            .unwrap();

        // Second session
        let dreamer2 = Dreamer::new(
            MockMemoryProvider::returning(vec![RawFact {
                category: "fact".into(),
                content: "second fact".into(),
                confidence: 1.0,
            }]),
            store.clone(),
        );
        let result = dreamer2
            .process_session("pr", "repo/1", "sess-2", &sample_transcript())
            .await
            .unwrap();

        assert_eq!(result.facts.len(), 2);
        assert_eq!(result.facts[0].source_session, "sess-1");
        assert_eq!(result.facts[1].source_session, "sess-2");
    }

    #[tokio::test]
    async fn process_session_does_not_persist_when_no_new_facts() {
        let store = MockMemoryStore::new();
        let dreamer = Dreamer::new(MockMemoryProvider::returning(vec![]), store.clone());

        let result = dreamer
            .process_session("agent", "proj/1", "sess-1", &[])
            .await
            .unwrap();

        assert!(result.facts.is_empty());

        // Nothing should be stored
        let client = MemoryClient::new(store);
        assert!(client.get("agent", "proj/1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn process_session_propagates_provider_error() {
        let dreamer = Dreamer::new(
            MockMemoryProvider::failing("LLM unavailable"),
            MockMemoryStore::new(),
        );
        let err = dreamer
            .process_session("agent", "proj/1", "sess-1", &sample_transcript())
            .await
            .unwrap_err();
        assert!(matches!(err, DreamerError::Llm(_)));
    }

    #[tokio::test]
    async fn process_session_empty_transcript_yields_no_facts_by_default() {
        let dreamer = Dreamer::new(MockMemoryProvider::returning(vec![]), MockMemoryStore::new());
        let result = dreamer
            .process_session("pr", "repo/2", "sess-1", &[])
            .await
            .unwrap();
        assert!(result.facts.is_empty());
    }

    #[tokio::test]
    async fn process_session_propagates_store_put_error() {
        #[derive(Clone)]
        struct FailingPutStore;

        #[derive(Debug)]
        struct PutErr;
        impl std::fmt::Display for PutErr {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "storage unavailable")
            }
        }
        impl std::error::Error for PutErr {}

        impl MemoryStore for FailingPutStore {
            type PutError = PutErr;
            type GetError = std::convert::Infallible;
            type DeleteError = std::convert::Infallible;

            async fn put(&self, _key: &str, _value: bytes::Bytes) -> Result<u64, PutErr> {
                Err(PutErr)
            }
            async fn get(
                &self,
                _key: &str,
            ) -> Result<Option<bytes::Bytes>, std::convert::Infallible> {
                Ok(None)
            }
            async fn delete(&self, _key: &str) -> Result<(), std::convert::Infallible> {
                Ok(())
            }
        }

        let provider = MockMemoryProvider::returning(vec![RawFact {
            category: "fact".into(),
            content: "some fact".into(),
            confidence: 0.9,
        }]);
        let dreamer = Dreamer::new(provider, FailingPutStore);
        let err = dreamer
            .process_session("agent", "proj/1", "sess-1", &sample_transcript())
            .await
            .unwrap_err();
        assert!(matches!(err, DreamerError::Store(_)));
    }
}
