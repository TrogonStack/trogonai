use async_trait::async_trait;

use crate::{ApiKeyHash, ApiKeyRecord, ApiKeyStoreError};

/// Resolves the record a hashed bearer token authenticates as.
#[async_trait]
pub trait ApiKeyStore: Send + Sync {
    /// The record stored under `key_hash`, or `None` when no such key exists.
    async fn lookup(&self, key_hash: &ApiKeyHash) -> Result<Option<ApiKeyRecord>, ApiKeyStoreError>;
}
