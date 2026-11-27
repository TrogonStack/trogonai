use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::Mutex;

use super::entry::Entry;
use super::operation_store::OperationStore;
use super::operation_store_error::OperationStoreError;

#[derive(Default)]
pub struct InMemoryOperationStore {
    entries: Mutex<HashMap<String, Entry>>,
    revision: AtomicU64,
}

impl InMemoryOperationStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn next_revision(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::SeqCst) + 1
    }
}

#[async_trait]
impl OperationStore for InMemoryOperationStore {
    async fn get(&self, key: &str) -> Result<Option<Entry>, OperationStoreError> {
        Ok(self.entries.lock().await.get(key).cloned())
    }

    async fn create(&self, key: &str, value: Bytes) -> Result<Option<u64>, OperationStoreError> {
        let mut entries = self.entries.lock().await;
        if entries.contains_key(key) {
            return Ok(None);
        }
        let revision = self.next_revision();
        entries.insert(key.to_string(), Entry { value, revision });
        Ok(Some(revision))
    }

    async fn update(&self, key: &str, value: Bytes, revision: u64) -> Result<Option<u64>, OperationStoreError> {
        let mut entries = self.entries.lock().await;
        match entries.get_mut(key) {
            Some(entry) if entry.revision == revision => {
                let next = self.next_revision();
                *entry = Entry { value, revision: next };
                Ok(Some(next))
            }
            _ => Ok(None),
        }
    }

    async fn delete(&self, key: &str, revision: Option<u64>) -> Result<bool, OperationStoreError> {
        let mut entries = self.entries.lock().await;
        match entries.get(key) {
            Some(entry) if revision.is_none_or(|r| r == entry.revision) => {
                entries.remove(key);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn keys(&self, prefix: &str) -> Result<Vec<String>, OperationStoreError> {
        let mut keys: Vec<String> = self
            .entries
            .lock()
            .await
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect();
        keys.sort();
        Ok(keys)
    }
}
