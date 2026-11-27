use std::time::{Duration, SystemTime};

use async_nats::jetstream;
use async_nats::jetstream::kv;
use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, TenantId};
use trogon_sandbox_protocol::subjects::LEASE_NAME_INDEX_KV_BUCKET;

use super::kv_keys::{legacy_named_lease_key, named_lease_key};
use super::stored_claim::StoredClaim;
use super::{NameClaim, NamedLeaseStore, NamedLeaseStoreError};

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(constant_outside_constants_module, reason = "the constant is private to this module")
)]
const CAS_ATTEMPTS: usize = 8;

pub struct NatsKvNamedLeaseStore {
    pub(super) kv: kv::Store,
}

impl NatsKvNamedLeaseStore {
    pub async fn new(jetstream: jetstream::Context) -> Result<Self, NamedLeaseStoreError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: LEASE_NAME_INDEX_KV_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(NamedLeaseStoreError::nats)?;
        Ok(Self { kv })
    }
}

#[async_trait]
impl NamedLeaseStore for NatsKvNamedLeaseStore {
    async fn claim(
        &self,
        tenant: &TenantId,
        name: &str,
        lease_id: &LeaseId,
        pending_ttl: Duration,
    ) -> Result<NameClaim, NamedLeaseStoreError> {
        let key = named_lease_key(tenant, name);
        let payload = StoredClaim::pending(lease_id, pending_ttl).encode()?;
        for _ in 0..CAS_ATTEMPTS {
            match self.kv.create(&key, payload.clone()).await {
                Ok(_) => return Ok(NameClaim::Claimed),
                Err(error) if error.kind() == kv::CreateErrorKind::AlreadyExists => {}
                Err(error) => return Err(NamedLeaseStoreError::nats(error)),
            }
            match self.kv.entry(&key).await.map_err(NamedLeaseStoreError::nats)? {
                Some(entry) if entry.operation == kv::Operation::Put => {
                    let current = StoredClaim::decode(&entry.value)?;
                    if current.lease() == lease_id {
                        return Ok(NameClaim::Claimed);
                    }
                    let now = SystemTime::now();
                    let age = now.duration_since(SystemTime::from(entry.created)).unwrap_or_default();
                    return Ok(NameClaim::Held {
                        pending: current.is_pending(now),
                        lease: current.lease().clone(),
                        age,
                    });
                }
                // Released between the create and the read: try to claim again.
                _ => continue,
            }
        }
        Err(NamedLeaseStoreError::ClaimRetriesExhausted)
    }

    async fn replace_if_matches(
        &self,
        tenant: &TenantId,
        name: &str,
        old: &LeaseId,
        new: &LeaseId,
        pending_ttl: Duration,
    ) -> Result<bool, NamedLeaseStoreError> {
        let key = named_lease_key(tenant, name);
        let Some(entry) = self.kv.entry(&key).await.map_err(NamedLeaseStoreError::nats)? else {
            return Ok(false);
        };
        if entry.operation != kv::Operation::Put {
            return Ok(false);
        }
        if StoredClaim::decode(&entry.value)?.lease() != old {
            return Ok(false);
        }
        let payload = StoredClaim::pending(new, pending_ttl).encode()?;
        match self.kv.update(&key, payload, entry.revision).await {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => Ok(false),
            Err(error) => Err(NamedLeaseStoreError::nats(error)),
        }
    }

    async fn confirm(&self, tenant: &TenantId, name: &str, lease_id: &LeaseId) -> Result<bool, NamedLeaseStoreError> {
        let key = named_lease_key(tenant, name);
        for _ in 0..CAS_ATTEMPTS {
            let Some(entry) = self.kv.entry(&key).await.map_err(NamedLeaseStoreError::nats)? else {
                return Ok(false);
            };
            if entry.operation != kv::Operation::Put {
                return Ok(false);
            }
            let current = StoredClaim::decode(&entry.value)?;
            if current.lease() != lease_id {
                return Ok(false);
            }
            if matches!(current, StoredClaim::Confirmed(_)) {
                return Ok(true);
            }
            let payload = StoredClaim::Confirmed(lease_id.clone()).encode()?;
            match self.kv.update(&key, payload, entry.revision).await {
                Ok(_) => return Ok(true),
                Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                Err(error) => return Err(NamedLeaseStoreError::nats(error)),
            }
        }
        Err(NamedLeaseStoreError::CasRetriesExhausted { operation: "confirm" })
    }

    async fn set(&self, tenant: &TenantId, name: &str, lease_id: &LeaseId) -> Result<(), NamedLeaseStoreError> {
        let key = named_lease_key(tenant, name);
        self.kv
            .put(key, StoredClaim::Confirmed(lease_id.clone()).encode()?)
            .await
            .map_err(NamedLeaseStoreError::nats)?;
        Ok(())
    }

    async fn get(&self, tenant: &TenantId, name: &str) -> Result<Option<LeaseId>, NamedLeaseStoreError> {
        let key = named_lease_key(tenant, name);
        if let Some(value) = self.kv.get(&key).await.map_err(NamedLeaseStoreError::nats)? {
            return Ok(Some(StoredClaim::decode(&value)?.lease().clone()));
        }
        let legacy = legacy_named_lease_key(tenant, name);
        if legacy == key {
            return Ok(None);
        }
        match self.kv.get(&legacy).await.map_err(NamedLeaseStoreError::nats)? {
            None => Ok(None),
            Some(value) => Ok(Some(StoredClaim::decode(&value)?.lease().clone())),
        }
    }

    async fn clear(&self, tenant: &TenantId, name: &str) -> Result<(), NamedLeaseStoreError> {
        let key = named_lease_key(tenant, name);
        self.kv.delete(&key).await.map_err(NamedLeaseStoreError::nats)?;
        let legacy = legacy_named_lease_key(tenant, name);
        if legacy != key
            && self
                .kv
                .get(&legacy)
                .await
                .map_err(NamedLeaseStoreError::nats)?
                .is_some()
        {
            self.kv.delete(&legacy).await.map_err(NamedLeaseStoreError::nats)?;
        }
        Ok(())
    }
    async fn clear_if_matches(
        &self,
        tenant: &TenantId,
        name: &str,
        lease: &LeaseId,
    ) -> Result<bool, NamedLeaseStoreError> {
        let key = named_lease_key(tenant, name);
        if self
            .clear_key_if_matches(&key, lease)
            .await
            .map_err(NamedLeaseStoreError::nats)?
        {
            return Ok(true);
        }
        let legacy = legacy_named_lease_key(tenant, name);
        if legacy == key {
            return Ok(false);
        }
        self.clear_key_if_matches(&legacy, lease).await
    }
}

impl NatsKvNamedLeaseStore {
    async fn clear_key_if_matches(&self, key: &str, lease: &LeaseId) -> Result<bool, NamedLeaseStoreError> {
        for _ in 0..CAS_ATTEMPTS {
            let Some(entry) = self.kv.entry(key).await.map_err(NamedLeaseStoreError::nats)? else {
                return Ok(false);
            };
            if entry.operation != kv::Operation::Put {
                return Ok(false);
            }
            if StoredClaim::decode(&entry.value)?.lease() != lease {
                return Ok(false);
            }
            match self.kv.delete_expect_revision(key, Some(entry.revision)).await {
                Ok(()) => return Ok(true),
                Err(error) if error.kind() == kv::DeleteErrorKind::WrongLastRevision => continue,
                Err(error) => return Err(NamedLeaseStoreError::nats(error)),
            }
        }
        Err(NamedLeaseStoreError::CasRetriesExhausted { operation: "release" })
    }
}
