use async_nats::jetstream;
use async_nats::jetstream::kv;
use async_trait::async_trait;
use bytes::Bytes;
use trogon_sandbox_contract::{LeaseGranted, LeaseId, LeaseRecord, LeaseRequestKey, LeaseStatus, TenantId};

use super::attempt::{attempt_key, storage_key, validate_attempt, validate_attempt_replacement};
use super::{IdempotencyStore, IdempotencyStoreError};
use crate::constants::IDEMPOTENCY_RECORD_MAX_AGE;

pub struct NatsKvIdempotencyStore {
    kv: kv::Store,
    pub(super) attempts: kv::Store,
}

impl NatsKvIdempotencyStore {
    pub async fn new(jetstream: jetstream::Context, bucket: &str) -> Result<Self, IdempotencyStoreError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: bucket.to_string(),
                history: 1,
                max_age: IDEMPOTENCY_RECORD_MAX_AGE,
                ..Default::default()
            })
            .await?;
        let attempts = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: format!("{bucket}-attempts"),
                history: 1,
                ..Default::default()
            })
            .await?;
        Ok(Self { kv, attempts })
    }

    async fn attempt_entry(
        &self,
        tenant: &TenantId,
        key: &LeaseRequestKey,
    ) -> Result<Option<(LeaseRecord, u64)>, IdempotencyStoreError> {
        let Some(entry) = self.attempts.entry(attempt_key(tenant, key)).await? else {
            return Ok(None);
        };
        if matches!(entry.operation, kv::Operation::Delete | kv::Operation::Purge) {
            return Ok(None);
        }
        let record = serde_json::from_slice(&entry.value)?;
        validate_attempt(tenant, key, &record)?;
        Ok(Some((record, entry.revision)))
    }
}

#[async_trait]
impl IdempotencyStore for NatsKvIdempotencyStore {
    async fn get(&self, tenant: &TenantId, key: &str) -> Result<Option<LeaseGranted>, IdempotencyStoreError> {
        let k = storage_key(tenant, key);
        match self.kv.entry(&k).await? {
            None => Ok(None),
            Some(entry) => {
                let granted: LeaseGranted = serde_json::from_slice(&entry.value)?;
                Ok(Some(granted))
            }
        }
    }

    async fn put_if_absent(
        &self,
        tenant: &TenantId,
        key: &str,
        response: &LeaseGranted,
    ) -> Result<Option<LeaseGranted>, IdempotencyStoreError> {
        let k = storage_key(tenant, key);
        let payload = serde_json::to_vec(response)?;
        match self.kv.create(&k, Bytes::from(payload)).await {
            Ok(_) => Ok(None),
            Err(error) if error.kind() == kv::CreateErrorKind::AlreadyExists => {
                match self.get(tenant, key).await? {
                    Some(existing) => Ok(Some(existing)),
                    // The winner expired between the two reads; the key is
                    // free again, so this grant takes it.
                    None => self.put_if_absent(tenant, key, response).await,
                }
            }
            Err(error) => Err(IdempotencyStoreError::from(error)),
        }
    }

    async fn get_attempt(
        &self,
        tenant: &TenantId,
        key: &LeaseRequestKey,
    ) -> Result<Option<LeaseRecord>, IdempotencyStoreError> {
        Ok(self.attempt_entry(tenant, key).await?.map(|(record, _)| record))
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
        match self
            .attempts
            .create(attempt_key(tenant, key), serde_json::to_vec(record)?.into())
            .await
        {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == kv::CreateErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(IdempotencyStoreError::from(error)),
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
        for _ in 0..8 {
            let Some((current, revision)) = self.attempt_entry(tenant, key).await? else {
                return Ok(false);
            };
            if &current != expected {
                return Ok(false);
            }
            match self
                .attempts
                .update(attempt_key(tenant, key), serde_json::to_vec(record)?.into(), revision)
                .await
            {
                Ok(_) => return Ok(true),
                Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                Err(error) => return Err(IdempotencyStoreError::from(error)),
            }
        }
        Err(IdempotencyStoreError::UpdateRetriesExhausted)
    }

    async fn forget_attempt(
        &self,
        tenant: &TenantId,
        key: &LeaseRequestKey,
        lease_id: &LeaseId,
    ) -> Result<bool, IdempotencyStoreError> {
        for _ in 0..8 {
            let Some((record, revision)) = self.attempt_entry(tenant, key).await? else {
                return Ok(false);
            };
            if record.lease_id != *lease_id || record.status != LeaseStatus::Pending {
                return Ok(false);
            }
            match self
                .attempts
                .delete_expect_revision(attempt_key(tenant, key), Some(revision))
                .await
            {
                Ok(_) => return Ok(true),
                Err(error) if error.kind() == kv::DeleteErrorKind::WrongLastRevision => continue,
                Err(error) => return Err(IdempotencyStoreError::from(error)),
            }
        }
        Err(IdempotencyStoreError::RetirementRetriesExhausted)
    }
}
