use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use tokio::sync::RwLock;
use trogon_sandbox_contract::{LeaseId, TenantId};

use super::kv_keys::named_lease_key;
use super::{NameClaim, NamedLeaseStore, NamedLeaseStoreError};

/// Lease id, creation time and optional expiry of one named lease.
type NamedLeaseRecord = (LeaseId, SystemTime, Option<SystemTime>);

pub struct InMemoryNamedLeaseStore {
    records: RwLock<HashMap<String, NamedLeaseRecord>>,
}

impl InMemoryNamedLeaseStore {
    pub fn new() -> Self {
        Self {
            records: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryNamedLeaseStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl NamedLeaseStore for InMemoryNamedLeaseStore {
    async fn claim(
        &self,
        tenant: &TenantId,
        name: &str,
        lease_id: &LeaseId,
        pending_ttl: Duration,
    ) -> Result<NameClaim, NamedLeaseStoreError> {
        let mut records = self.records.write().await;
        let key = named_lease_key(tenant, name);
        let now = SystemTime::now();
        match records.get(&key) {
            None => {
                records.insert(key, (lease_id.clone(), now, Some(now + pending_ttl)));
                Ok(NameClaim::Claimed)
            }
            Some((current, _, _)) if current == lease_id => Ok(NameClaim::Claimed),
            Some((current, written, pending_until)) => Ok(NameClaim::Held {
                lease: current.clone(),
                age: now.duration_since(*written).unwrap_or_default(),
                pending: pending_until.is_some_and(|until| until > now),
            }),
        }
    }

    async fn replace_if_matches(
        &self,
        tenant: &TenantId,
        name: &str,
        old: &LeaseId,
        new: &LeaseId,
        pending_ttl: Duration,
    ) -> Result<bool, NamedLeaseStoreError> {
        let mut records = self.records.write().await;
        let key = named_lease_key(tenant, name);
        match records.get(&key) {
            Some((current, _, _)) if current == old => {
                let now = SystemTime::now();
                records.insert(key, (new.clone(), now, Some(now + pending_ttl)));
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn confirm(&self, tenant: &TenantId, name: &str, lease_id: &LeaseId) -> Result<bool, NamedLeaseStoreError> {
        let mut records = self.records.write().await;
        match records.get_mut(&named_lease_key(tenant, name)) {
            Some((current, _, pending_until)) if current == lease_id => {
                *pending_until = None;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn set(&self, tenant: &TenantId, name: &str, lease_id: &LeaseId) -> Result<(), NamedLeaseStoreError> {
        self.records.write().await.insert(
            named_lease_key(tenant, name),
            (lease_id.clone(), SystemTime::now(), None),
        );
        Ok(())
    }

    async fn get(&self, tenant: &TenantId, name: &str) -> Result<Option<LeaseId>, NamedLeaseStoreError> {
        Ok(self
            .records
            .read()
            .await
            .get(&named_lease_key(tenant, name))
            .map(|(lease, _, _)| lease.clone()))
    }

    async fn clear(&self, tenant: &TenantId, name: &str) -> Result<(), NamedLeaseStoreError> {
        self.records.write().await.remove(&named_lease_key(tenant, name));
        Ok(())
    }
    async fn clear_if_matches(
        &self,
        tenant: &TenantId,
        name: &str,
        lease: &LeaseId,
    ) -> Result<bool, NamedLeaseStoreError> {
        let mut records = self.records.write().await;
        let key = named_lease_key(tenant, name);
        if records.get(&key).map(|(current, _, _)| current) != Some(lease) {
            return Ok(false);
        }
        records.remove(&key);
        Ok(true)
    }
}
