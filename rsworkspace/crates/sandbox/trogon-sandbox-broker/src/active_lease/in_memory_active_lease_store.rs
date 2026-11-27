use std::collections::HashMap;
use std::time::SystemTime;

use async_trait::async_trait;
use tokio::sync::Mutex;
use trogon_sandbox_contract::{LeaseId, LeaseRecord};

use super::{ActiveLeaseStore, ActiveLeaseStoreError};

pub struct InMemoryActiveLeaseStore {
    records: Mutex<HashMap<String, LeaseRecord>>,
}

impl InMemoryActiveLeaseStore {
    pub fn new() -> Self {
        Self {
            records: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryActiveLeaseStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ActiveLeaseStore for InMemoryActiveLeaseStore {
    async fn put(&self, record: &LeaseRecord) -> Result<(), ActiveLeaseStoreError> {
        self.records
            .lock()
            .await
            .insert(record.lease_id.as_str().to_owned(), record.clone());
        Ok(())
    }

    async fn get(&self, lease_id: &LeaseId) -> Result<Option<LeaseRecord>, ActiveLeaseStoreError> {
        Ok(self.records.lock().await.get(lease_id.as_str()).cloned())
    }

    async fn delete(&self, lease_id: &LeaseId) -> Result<bool, ActiveLeaseStoreError> {
        Ok(self.records.lock().await.remove(lease_id.as_str()).is_some())
    }

    async fn all(&self) -> Result<Vec<LeaseRecord>, ActiveLeaseStoreError> {
        Ok(self.records.lock().await.values().cloned().collect())
    }

    async fn touch(&self, lease_id: &LeaseId, at: SystemTime) -> Result<bool, ActiveLeaseStoreError> {
        let mut records = self.records.lock().await;
        let Some(record) = records.get_mut(lease_id.as_str()) else {
            return Ok(false);
        };
        if record.last_activity_at.is_none_or(|seen| seen < at) {
            record.last_activity_at = Some(at);
        }
        Ok(true)
    }

    async fn replace_if_unchanged(
        &self,
        expected: &LeaseRecord,
        record: &LeaseRecord,
    ) -> Result<bool, ActiveLeaseStoreError> {
        if expected.lease_id != record.lease_id {
            return Err(ActiveLeaseStoreError::LeaseIdChanged);
        }
        let mut records = self.records.lock().await;
        match records.get_mut(expected.lease_id.as_str()) {
            Some(current) if current == expected => {
                *current = record.clone();
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
