use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::Mutex;
use trogon_sandbox_contract::{LeaseGranted, LeaseId, LeaseRecord, LeaseRequestKey, LeaseStatus, TenantId};

use super::attempt::{attempt_key, storage_key, validate_attempt, validate_attempt_replacement};
use super::{IdempotencyStore, IdempotencyStoreError};
use crate::constants::IDEMPOTENCY_RECORD_MAX_AGE;

pub struct InMemoryIdempotencyStore {
    max_age: Duration,
    records: Mutex<HashMap<String, (LeaseGranted, Instant)>>,
    attempts: Mutex<HashMap<String, LeaseRecord>>,
}

impl InMemoryIdempotencyStore {
    pub fn new() -> Self {
        Self::with_max_age(IDEMPOTENCY_RECORD_MAX_AGE)
    }

    /// Expires each granted reply `max_age` after it was stored.
    pub fn with_max_age(max_age: Duration) -> Self {
        Self {
            max_age,
            records: Mutex::new(HashMap::new()),
            attempts: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryIdempotencyStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl IdempotencyStore for InMemoryIdempotencyStore {
    async fn get(&self, tenant: &TenantId, key: &str) -> Result<Option<LeaseGranted>, IdempotencyStoreError> {
        let k = storage_key(tenant, key);
        let mut records = self.records.lock().await;
        let max_age = self.max_age;
        records.retain(|_, (_, stored)| stored.elapsed() <= max_age);
        Ok(records.get(&k).map(|(granted, _)| granted.clone()))
    }

    async fn put_if_absent(
        &self,
        tenant: &TenantId,
        key: &str,
        response: &LeaseGranted,
    ) -> Result<Option<LeaseGranted>, IdempotencyStoreError> {
        let k = storage_key(tenant, key);
        let mut records = self.records.lock().await;
        let max_age = self.max_age;
        records.retain(|_, (_, stored)| stored.elapsed() <= max_age);
        if let Some((existing, _)) = records.get(&k) {
            return Ok(Some(existing.clone()));
        }
        records.insert(k, (response.clone(), Instant::now()));
        Ok(None)
    }

    async fn get_attempt(
        &self,
        tenant: &TenantId,
        key: &LeaseRequestKey,
    ) -> Result<Option<LeaseRecord>, IdempotencyStoreError> {
        Ok(self.attempts.lock().await.get(&attempt_key(tenant, key)).cloned())
    }

    async fn begin_attempt(
        &self,
        tenant: &TenantId,
        key: &LeaseRequestKey,
        record: &LeaseRecord,
    ) -> Result<bool, IdempotencyStoreError> {
        validate_attempt(tenant, key, record)?;
        if record.status != LeaseStatus::Pending {
            return Err(IdempotencyStoreError::NewDispatchNotPending);
        }
        let mut attempts = self.attempts.lock().await;
        match attempts.entry(attempt_key(tenant, key)) {
            std::collections::hash_map::Entry::Occupied(_) => Ok(false),
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(record.clone());
                Ok(true)
            }
        }
    }

    async fn replace_attempt(
        &self,
        tenant: &TenantId,
        key: &LeaseRequestKey,
        expected: &LeaseRecord,
        record: &LeaseRecord,
    ) -> Result<bool, IdempotencyStoreError> {
        validate_attempt(tenant, key, record)?;
        validate_attempt_replacement(expected, record)?;
        let mut attempts = self.attempts.lock().await;
        let Some(current) = attempts.get_mut(&attempt_key(tenant, key)) else {
            return Ok(false);
        };
        if current != expected {
            return Ok(false);
        }
        *current = record.clone();
        Ok(true)
    }

    async fn forget_attempt(
        &self,
        tenant: &TenantId,
        key: &LeaseRequestKey,
        lease_id: &LeaseId,
    ) -> Result<bool, IdempotencyStoreError> {
        let mut attempts = self.attempts.lock().await;
        let storage_key = attempt_key(tenant, key);
        if !attempts
            .get(&storage_key)
            .is_some_and(|record| record.lease_id == *lease_id && record.status == LeaseStatus::Pending)
        {
            return Ok(false);
        }
        attempts.remove(&storage_key);
        Ok(true)
    }
}
