use async_trait::async_trait;

use crate::{SessionRecord, SessionStoreError};

/// Persists session records under the hash of their token.
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn put(&self, token_hash: &str, record: SessionRecord) -> Result<(), SessionStoreError>;
    async fn lookup(&self, token_hash: &str) -> Result<Option<SessionRecord>, SessionStoreError>;
}
