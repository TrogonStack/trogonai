use std::collections::HashMap;

use tokio::sync::Mutex;

use crate::template::{TemplateIndex, TemplateIndexError, TemplateRecord};

/// Which operations of the index should fail.
#[derive(Clone, Copy, Default)]
pub(in crate::template) struct Faults {
    pub(in crate::template) get: bool,
    pub(in crate::template) create: bool,
    pub(in crate::template) put: bool,
    pub(in crate::template) delete: bool,
    /// `create` reports that someone else holds the key, though `get` saw nothing.
    pub(in crate::template) lose_claim: bool,
}

#[derive(Default)]
pub(in crate::template) struct MemoryIndex {
    entries: Mutex<HashMap<String, TemplateRecord>>,
    faults: Faults,
}

fn down() -> TemplateIndexError {
    TemplateIndexError::storage(std::io::Error::other("index down"))
}

impl MemoryIndex {
    pub(in crate::template) fn with_faults(faults: Faults) -> Self {
        Self {
            faults,
            ..Self::default()
        }
    }

    pub(in crate::template) async fn record(&self, key: &str) -> Option<TemplateRecord> {
        self.entries.lock().await.get(key).cloned()
    }

    pub(in crate::template) async fn seed(&self, key: &str, record: TemplateRecord) {
        self.entries.lock().await.insert(key.to_string(), record);
    }
}

#[async_trait::async_trait]
impl TemplateIndex for MemoryIndex {
    async fn get(&self, key: &str) -> Result<Option<TemplateRecord>, TemplateIndexError> {
        if self.faults.get {
            return Err(down());
        }
        Ok(self.record(key).await)
    }

    async fn create(&self, key: &str, record: &TemplateRecord) -> Result<bool, TemplateIndexError> {
        if self.faults.create {
            return Err(down());
        }
        if self.faults.lose_claim {
            return Ok(false);
        }
        let mut entries = self.entries.lock().await;
        if entries.contains_key(key) {
            return Ok(false);
        }
        entries.insert(key.to_string(), record.clone());
        Ok(true)
    }

    async fn put(&self, key: &str, record: &TemplateRecord) -> Result<(), TemplateIndexError> {
        if self.faults.put {
            return Err(down());
        }
        self.seed(key, record.clone()).await;
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), TemplateIndexError> {
        if self.faults.delete {
            return Err(down());
        }
        self.entries.lock().await.remove(key);
        Ok(())
    }
}
