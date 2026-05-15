use futures_util::future::BoxFuture;

use crate::{entry::TranscriptEntry, error::TranscriptError, store::TranscriptStore};

/// Read access to the transcript store.
///
/// Object-safe so it can be stored as `Arc<dyn TranscriptRead>` without adding
/// a type parameter to every struct that needs it.
pub trait TranscriptRead: Send + Sync {
    fn query_entity<'a>(
        &'a self,
        actor_type: &'a str,
        actor_key: &'a str,
    ) -> BoxFuture<'a, Result<Vec<TranscriptEntry>, TranscriptError>>;
}

/// Production implementation backed by a real NATS JetStream context.
#[derive(Clone)]
pub struct NatsTranscriptReader {
    store: TranscriptStore,
}

impl NatsTranscriptReader {
    pub fn new(js: async_nats::jetstream::Context) -> Self {
        Self {
            store: TranscriptStore::new(js),
        }
    }
}

impl TranscriptRead for NatsTranscriptReader {
    fn query_entity<'a>(
        &'a self,
        actor_type: &'a str,
        actor_key: &'a str,
    ) -> BoxFuture<'a, Result<Vec<TranscriptEntry>, TranscriptError>> {
        Box::pin(self.store.query(actor_type, actor_key))
    }
}

/// In-memory reader for unit tests.
#[cfg(any(test, feature = "test-support"))]
pub mod mock {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    pub struct MockTranscriptReader {
        entries: Arc<Mutex<Vec<TranscriptEntry>>>,
    }

    impl MockTranscriptReader {
        pub fn new() -> Self {
            Self::default()
        }

        /// Pre-load entries that will be returned by every `query_entity` call.
        pub fn seed(&self, entries: Vec<TranscriptEntry>) {
            *self.entries.lock().unwrap() = entries;
        }
    }

    impl TranscriptRead for MockTranscriptReader {
        fn query_entity<'a>(
            &'a self,
            _actor_type: &'a str,
            _actor_key: &'a str,
        ) -> BoxFuture<'a, Result<Vec<TranscriptEntry>, TranscriptError>> {
            let entries = self.entries.lock().unwrap().clone();
            Box::pin(std::future::ready(Ok(entries)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::MockTranscriptReader;
    use super::TranscriptRead;
    use crate::entry::{Role, TranscriptEntry};

    fn user_msg(content: &str) -> TranscriptEntry {
        TranscriptEntry::Message {
            role: Role::User,
            content: content.to_string(),
            timestamp: 1_000,
            tokens: None,
        }
    }

    #[tokio::test]
    async fn mock_returns_empty_before_seed() {
        let reader = MockTranscriptReader::new();
        let entries = reader.query_entity("pr", "repo/1").await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn mock_returns_seeded_entries() {
        let reader = MockTranscriptReader::new();
        reader.seed(vec![user_msg("hello"), user_msg("world")]);
        let entries = reader.query_entity("pr", "repo/1").await.unwrap();
        assert_eq!(entries.len(), 2);
        assert!(matches!(
            &entries[0],
            TranscriptEntry::Message { role: Role::User, content, .. } if content == "hello"
        ));
    }

    #[tokio::test]
    async fn mock_ignores_actor_type_and_key() {
        let reader = MockTranscriptReader::new();
        reader.seed(vec![user_msg("x")]);
        // Different actor_type and actor_key still return seeded entries.
        let a = reader.query_entity("pr", "repo/1").await.unwrap();
        let b = reader.query_entity("incident", "prod/42").await.unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
    }

    #[tokio::test]
    async fn mock_seed_replaces_previous_entries() {
        let reader = MockTranscriptReader::new();
        reader.seed(vec![user_msg("first")]);
        reader.seed(vec![user_msg("second"), user_msg("third")]);
        let entries = reader.query_entity("pr", "repo/1").await.unwrap();
        assert_eq!(entries.len(), 2);
        assert!(matches!(
            &entries[0],
            TranscriptEntry::Message { content, .. } if content == "second"
        ));
    }
}
