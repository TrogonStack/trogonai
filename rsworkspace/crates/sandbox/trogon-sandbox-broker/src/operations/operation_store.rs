use async_trait::async_trait;
use bytes::Bytes;

use super::entry::Entry;
use super::operation_store_error::OperationStoreError;

/// Compare-and-swap key-value access the operation ledger is built on.
#[async_trait]
pub trait OperationStore: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<Entry>, OperationStoreError>;
    /// Writes a new key; `None` when it already exists.
    async fn create(&self, key: &str, value: Bytes) -> Result<Option<u64>, OperationStoreError>;
    /// Replaces a key at `revision`; `None` when the revision moved on.
    async fn update(&self, key: &str, value: Bytes, revision: u64) -> Result<Option<u64>, OperationStoreError>;
    /// Removes a key, at `revision` when given. `false` when nothing matched.
    async fn delete(&self, key: &str, revision: Option<u64>) -> Result<bool, OperationStoreError>;
    async fn keys(&self, prefix: &str) -> Result<Vec<String>, OperationStoreError>;
}
