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
