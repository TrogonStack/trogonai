use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::{SessionRecord, SessionStore, SessionStoreError};

/// A [`SessionStore`] that keeps records in process memory.
#[derive(Default)]
pub struct InMemorySessionStore {
    sessions: RwLock<HashMap<String, SessionRecord>>,
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn put(&self, token_hash: &str, record: SessionRecord) -> Result<(), SessionStoreError> {
        self.sessions.write().await.insert(token_hash.to_string(), record);
        Ok(())
    }

    async fn lookup(&self, token_hash: &str) -> Result<Option<SessionRecord>, SessionStoreError> {
        let record = self.sessions.read().await.get(token_hash).cloned();
        Ok(record.filter(|record| !record.is_expired()))
    }
}
