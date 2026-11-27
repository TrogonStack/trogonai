use std::collections::BTreeMap;

use tokio::sync::Mutex;

use super::exchange_key::exchange_key;
use super::{ActorEntry, ActorIndex, ActorIndexResult, entry_key};

#[derive(Default)]
pub(crate) struct MemoryActorIndex {
    entries: Mutex<BTreeMap<String, ActorEntry>>,
}

#[async_trait::async_trait]
impl ActorIndex for MemoryActorIndex {
    async fn put(&self, entry: &ActorEntry) -> ActorIndexResult<()> {
        self.entries.lock().await.insert(entry.key(), entry.clone());
        Ok(())
    }

    async fn get(&self, atespace: &str, actor: &str) -> ActorIndexResult<Option<ActorEntry>> {
        Ok(self.entries.lock().await.get(&entry_key(atespace, actor)).cloned())
    }

    async fn delete(&self, atespace: &str, actor: &str) -> ActorIndexResult<()> {
        self.entries.lock().await.remove(&entry_key(atespace, actor));
        Ok(())
    }

    async fn all(&self) -> ActorIndexResult<Vec<ActorEntry>> {
        Ok(self.entries.lock().await.values().cloned().collect())
    }

    async fn compare_exchange(
        &self,
        expected: Option<&ActorEntry>,
        replacement: Option<&ActorEntry>,
    ) -> ActorIndexResult<bool> {
        let key = exchange_key(expected, replacement)?;
        let mut entries = self.entries.lock().await;
        if entries.get(&key) != expected {
            return Ok(false);
        }
        match replacement {
            Some(entry) => {
                entries.insert(key, entry.clone());
            }
            None => {
                entries.remove(&key);
            }
        }
        Ok(true)
    }
}
