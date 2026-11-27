use std::time::Duration;

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, TenantId};

use super::{NameClaim, NamedLeaseStoreError};

#[async_trait]
pub trait NamedLeaseStore: Send + Sync {
    /// Atomically maps `name` to `lease_id` only if the name is free. Exactly
    /// one of any number of concurrent claimers observes `Claimed`. The new
    /// mapping is written pending for `pending_ttl`; the claimer calls
    /// [`Self::confirm`] once the lease is recorded.
    async fn claim(
        &self,
        tenant: &TenantId,
        name: &str,
        lease_id: &LeaseId,
        pending_ttl: Duration,
    ) -> Result<NameClaim, NamedLeaseStoreError>;
    /// Atomically re-points `name` from `old` to `new`, pending for
    /// `pending_ttl`. False when the mapping no longer names `old`.
    async fn replace_if_matches(
        &self,
        tenant: &TenantId,
        name: &str,
        old: &LeaseId,
        new: &LeaseId,
        pending_ttl: Duration,
    ) -> Result<bool, NamedLeaseStoreError>;
    /// Turns a pending claim by `lease_id` into a confirmed mapping. False
    /// when the name no longer maps to `lease_id`.
    async fn confirm(
        &self,
        _tenant: &TenantId,
        _name: &str,
        _lease_id: &LeaseId,
    ) -> Result<bool, NamedLeaseStoreError> {
        Ok(true)
    }
    async fn set(&self, tenant: &TenantId, name: &str, lease_id: &LeaseId) -> Result<(), NamedLeaseStoreError>;
    async fn get(&self, tenant: &TenantId, name: &str) -> Result<Option<LeaseId>, NamedLeaseStoreError>;
    /// Frees `name` so a future lease request can reconnect-or-resume under it.
    async fn clear(&self, tenant: &TenantId, name: &str) -> Result<(), NamedLeaseStoreError>;
    async fn clear_if_matches(
        &self,
        _tenant: &TenantId,
        _name: &str,
        _lease: &LeaseId,
    ) -> Result<bool, NamedLeaseStoreError> {
        Err(NamedLeaseStoreError::ExactOwnerReleaseUnavailable)
    }
}
