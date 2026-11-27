use std::collections::HashSet;

use async_nats::jetstream::{self, kv};
use async_trait::async_trait;
use bytes::Bytes;
use trogon_sandbox_contract::{LeaseId, PullSecretId, TenantId};
use trogon_sandbox_protocol::COMMITTED_POLICY_BUCKET;

use crate::constants::DEFAULT_COMMIT_ATTEMPTS;
use crate::stored_policy::StoredPolicy;
use crate::{BackendError, CommittedPolicy, CommittedPolicyStore, CommittedPolicyWriter, PolicyStoreError};

/// Committed policies in a NATS KV bucket, one key per tenant and lease.
#[derive(Clone)]
pub struct NatsKvCommittedPolicies {
    kv: kv::Store,
    commit_attempts: usize,
}

impl NatsKvCommittedPolicies {
    pub fn new(kv: kv::Store) -> Self {
        Self {
            kv,
            commit_attempts: DEFAULT_COMMIT_ATTEMPTS,
        }
    }

    /// Sets how many times a commit retries after losing a race for the same lease.
    pub fn with_commit_attempts(mut self, attempts: usize) -> Self {
        self.commit_attempts = attempts;
        self
    }

    /// Creates the committed-policy bucket when it does not exist yet and opens it.
    pub async fn open_bucket(jetstream: &jetstream::Context) -> Result<kv::Store, PolicyStoreError> {
        let store = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: COMMITTED_POLICY_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(BackendError::new)?;
        Ok(store)
    }

    /// Hex keeps any tenant or lease id a valid KV key and keeps one tenant's ids from running into another's.
    fn key(tenant: &TenantId, lease: &LeaseId) -> String {
        format!("{}.{}", hex::encode(tenant.as_str()), hex::encode(lease.as_str()))
    }

    async fn live_entry(&self, key: &str) -> Result<Option<kv::Entry>, PolicyStoreError> {
        let entry = self.kv.entry(key).await.map_err(BackendError::new)?;
        Ok(entry.filter(|entry| matches!(entry.operation, kv::Operation::Put)))
    }
}

#[async_trait]
impl CommittedPolicyStore for NatsKvCommittedPolicies {
    async fn committed(&self, tenant: &TenantId, lease: &LeaseId) -> Result<Option<CommittedPolicy>, PolicyStoreError> {
        match self.live_entry(&Self::key(tenant, lease)).await? {
            Some(entry) => Ok(Some(
                serde_json::from_slice::<StoredPolicy>(&entry.value)?.into_committed(),
            )),
            None => Ok(None),
        }
    }
}

#[async_trait]
impl CommittedPolicyWriter for NatsKvCommittedPolicies {
    async fn commit(
        &self,
        tenant: &TenantId,
        lease: &LeaseId,
        references: HashSet<PullSecretId>,
    ) -> Result<u64, PolicyStoreError> {
        let key = Self::key(tenant, lease);
        for _ in 0..self.commit_attempts {
            let current = self.live_entry(&key).await?;
            let (revision, sequence) = match &current {
                Some(entry) => {
                    let committed = serde_json::from_slice::<StoredPolicy>(&entry.value)?.into_committed();
                    if committed.references == references {
                        return Ok(committed.revision);
                    }
                    (committed.revision + 1, Some(entry.revision))
                }
                None => (1, None),
            };
            let mut names: Vec<PullSecretId> = references.iter().cloned().collect();
            names.sort();
            let payload = Bytes::from(serde_json::to_vec(&StoredPolicy {
                revision,
                references: names,
            })?);
            let written = match sequence {
                Some(sequence) => self.kv.update(&key, payload, sequence).await.is_ok(),
                None => self.kv.create(&key, payload).await.is_ok(),
            };
            if written {
                return Ok(revision);
            }
        }
        Err(PolicyStoreError::Contended)
    }

    async fn revoke(&self, tenant: &TenantId, lease: &LeaseId) -> Result<(), PolicyStoreError> {
        self.kv
            .delete(Self::key(tenant, lease))
            .await
            .map_err(BackendError::new)?;
        Ok(())
    }
}
