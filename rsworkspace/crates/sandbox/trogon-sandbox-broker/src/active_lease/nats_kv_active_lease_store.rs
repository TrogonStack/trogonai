use std::time::SystemTime;

use async_nats::jetstream;
use async_nats::jetstream::kv;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use trogon_sandbox_contract::{LeaseId, LeaseRecord};

use super::{ActiveLeaseStore, ActiveLeaseStoreError};

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(constant_outside_constants_module, reason = "the constant is private to this module")
)]
const CAS_ATTEMPTS: usize = 8;

fn nats_kv_key(lease_id: &LeaseId) -> String {
    lease_id.as_str().replace(':', "_")
}

pub struct NatsKvActiveLeaseStore {
    kv: kv::Store,
}

impl NatsKvActiveLeaseStore {
    pub async fn new(jetstream: jetstream::Context, bucket: &str) -> Result<Self, ActiveLeaseStoreError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: bucket.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(ActiveLeaseStoreError::nats)?;
        Ok(Self { kv })
    }
}

#[async_trait]
impl ActiveLeaseStore for NatsKvActiveLeaseStore {
    async fn put(&self, record: &LeaseRecord) -> Result<(), ActiveLeaseStoreError> {
        let key = nats_kv_key(&record.lease_id);
        let payload = serde_json::to_vec(record)?;
        self.kv
            .put(key, Bytes::from(payload))
            .await
            .map_err(ActiveLeaseStoreError::nats)?;
        Ok(())
    }

    async fn get(&self, lease_id: &LeaseId) -> Result<Option<LeaseRecord>, ActiveLeaseStoreError> {
        let key = nats_kv_key(lease_id);
        match self.kv.get(&key).await.map_err(ActiveLeaseStoreError::nats)? {
            None => Ok(None),
            Some(value) => {
                let record: LeaseRecord = serde_json::from_slice(&value)?;
                Ok(Some(record))
            }
        }
    }

    async fn delete(&self, lease_id: &LeaseId) -> Result<bool, ActiveLeaseStoreError> {
        let key = nats_kv_key(lease_id);
        for _ in 0..CAS_ATTEMPTS {
            let Some(entry) = self.kv.entry(&key).await.map_err(ActiveLeaseStoreError::nats)? else {
                return Ok(false);
            };
            if entry.operation != kv::Operation::Put {
                return Ok(false);
            }
            // Revision-checked so exactly one concurrent deleter wins.
            match self.kv.delete_expect_revision(&key, Some(entry.revision)).await {
                Ok(()) => return Ok(true),
                Err(error) if error.kind() == kv::DeleteErrorKind::WrongLastRevision => continue,
                Err(error) => return Err(ActiveLeaseStoreError::nats(error)),
            }
        }
        Err(ActiveLeaseStoreError::CasRetriesExhausted { operation: "delete" })
    }

    async fn all(&self) -> Result<Vec<LeaseRecord>, ActiveLeaseStoreError> {
        let keys: Vec<String> = self
            .kv
            .keys()
            .await
            .map_err(ActiveLeaseStoreError::nats)?
            .filter_map(|r| async move { r.ok() })
            .collect()
            .await;

        let mut records = Vec::with_capacity(keys.len());
        for key in keys {
            let entry = match self.kv.entry(&key).await.map_err(ActiveLeaseStoreError::nats)? {
                Some(e) => e,
                None => continue,
            };
            if let Ok(record) = serde_json::from_slice::<LeaseRecord>(&entry.value) {
                records.push(record);
            }
        }
        Ok(records)
    }

    async fn touch(&self, lease_id: &LeaseId, at: SystemTime) -> Result<bool, ActiveLeaseStoreError> {
        let key = nats_kv_key(lease_id);
        for _ in 0..CAS_ATTEMPTS {
            let Some(entry) = self.kv.entry(&key).await.map_err(ActiveLeaseStoreError::nats)? else {
                return Ok(false);
            };
            if entry.operation != kv::Operation::Put {
                return Ok(false);
            }
            let mut record: LeaseRecord = serde_json::from_slice(&entry.value)?;
            if record.last_activity_at.is_some_and(|seen| seen >= at) {
                return Ok(true);
            }
            record.last_activity_at = Some(at);
            let payload = serde_json::to_vec(&record)?;
            // Revision-checked so a concurrent writer's change is never overwritten.
            match self.kv.update(&key, Bytes::from(payload), entry.revision).await {
                Ok(_) => return Ok(true),
                Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                Err(error) => return Err(ActiveLeaseStoreError::nats(error)),
            }
        }
        Err(ActiveLeaseStoreError::CasRetriesExhausted { operation: "touch" })
    }

    async fn replace_if_unchanged(
        &self,
        expected: &LeaseRecord,
        record: &LeaseRecord,
    ) -> Result<bool, ActiveLeaseStoreError> {
        if expected.lease_id != record.lease_id {
            return Err(ActiveLeaseStoreError::LeaseIdChanged);
        }
        let key = nats_kv_key(&expected.lease_id);
        let Some(entry) = self.kv.entry(&key).await.map_err(ActiveLeaseStoreError::nats)? else {
            return Ok(false);
        };
        if entry.operation != kv::Operation::Put {
            return Ok(false);
        }
        let current: LeaseRecord = serde_json::from_slice(&entry.value)?;
        if &current != expected {
            return Ok(false);
        }
        // The revision pins the comparison above to the write.
        match self
            .kv
            .update(&key, Bytes::from(serde_json::to_vec(record)?), entry.revision)
            .await
        {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => Ok(false),
            Err(error) => Err(ActiveLeaseStoreError::nats(error)),
        }
    }
}
