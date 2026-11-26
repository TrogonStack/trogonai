use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, PullSecretId, TenantId};

use crate::{CommittedPolicy, CommittedPolicyStore, CommittedPolicyWriter, PolicyStoreError, StaleCommit};

type PolicyKey = (TenantId, LeaseId);

/// A committed-policy record held in memory.
#[derive(Default)]
pub struct InMemoryCommittedPolicies {
    policies: Mutex<HashMap<PolicyKey, CommittedPolicy>>,
}

impl InMemoryCommittedPolicies {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<PolicyKey, CommittedPolicy>> {
        self.policies.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Commits exactly `revision` for the lease. A revision that is not newer than the committed one is refused,
    /// so a replayed commit cannot roll the policy back.
    pub fn commit_revision(
        &self,
        tenant: TenantId,
        lease: LeaseId,
        revision: u64,
        references: impl IntoIterator<Item = PullSecretId>,
    ) -> Result<(), StaleCommit> {
        let mut policies = self.lock();
        let key = (tenant, lease);
        if let Some(current) = policies.get(&key)
            && revision <= current.revision
        {
            return Err(StaleCommit {
                current: current.revision,
                offered: revision,
            });
        }
        policies.insert(
            key,
            CommittedPolicy {
                revision,
                references: references.into_iter().collect(),
            },
        );
        Ok(())
    }

    /// Forgets the lease, so nothing resolves for it any more.
    pub fn forget(&self, tenant: &TenantId, lease: &LeaseId) {
        self.lock().remove(&(tenant.clone(), lease.clone()));
    }
}

#[async_trait]
impl CommittedPolicyStore for InMemoryCommittedPolicies {
    async fn committed(&self, tenant: &TenantId, lease: &LeaseId) -> Result<Option<CommittedPolicy>, PolicyStoreError> {
        Ok(self.lock().get(&(tenant.clone(), lease.clone())).cloned())
    }
}

#[async_trait]
impl CommittedPolicyWriter for InMemoryCommittedPolicies {
    async fn commit(
        &self,
        tenant: &TenantId,
        lease: &LeaseId,
        references: HashSet<PullSecretId>,
    ) -> Result<u64, PolicyStoreError> {
        let revision = match self.committed(tenant, lease).await? {
            Some(current) if current.references == references => return Ok(current.revision),
            Some(current) => current.revision + 1,
            None => 1,
        };
        self.commit_revision(tenant.clone(), lease.clone(), revision, references)?;
        Ok(revision)
    }

    async fn revoke(&self, tenant: &TenantId, lease: &LeaseId) -> Result<(), PolicyStoreError> {
        self.forget(tenant, lease);
        Ok(())
    }
}
