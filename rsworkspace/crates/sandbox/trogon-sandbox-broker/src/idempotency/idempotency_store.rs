use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseGranted, LeaseId, LeaseRecord, LeaseRequestKey, TenantId};

use super::IdempotencyStoreError;

#[async_trait]
pub trait IdempotencyStore: Send + Sync {
    async fn get(&self, tenant: &TenantId, key: &str) -> Result<Option<LeaseGranted>, IdempotencyStoreError>;

    /// Records `response` as the grant for `key` unless another grant already
    /// holds it. Returns that earlier grant when it does, so two concurrent
    /// requests with one key agree on a single lease.
    async fn put_if_absent(
        &self,
        tenant: &TenantId,
        key: &str,
        response: &LeaseGranted,
    ) -> Result<Option<LeaseGranted>, IdempotencyStoreError>;

    async fn get_attempt(
        &self,
        _tenant: &TenantId,
        _key: &LeaseRequestKey,
    ) -> Result<Option<LeaseRecord>, IdempotencyStoreError> {
        Ok(None)
    }
    async fn begin_attempt(
        &self,
        _tenant: &TenantId,
        _key: &LeaseRequestKey,
        _record: &LeaseRecord,
    ) -> Result<bool, IdempotencyStoreError> {
        Err(IdempotencyStoreError::JournalUnavailable)
    }
    async fn replace_attempt(
        &self,
        _tenant: &TenantId,
        _key: &LeaseRequestKey,
        _expected: &LeaseRecord,
        _record: &LeaseRecord,
    ) -> Result<bool, IdempotencyStoreError> {
        Err(IdempotencyStoreError::JournalCasUnavailable)
    }
    async fn forget_attempt(
        &self,
        _tenant: &TenantId,
        _key: &LeaseRequestKey,
        _lease_id: &LeaseId,
    ) -> Result<bool, IdempotencyStoreError> {
        Err(IdempotencyStoreError::JournalRetirementUnavailable)
    }
}
