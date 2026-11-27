use std::time::SystemTime;

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, LeaseRecord};

use super::ActiveLeaseStoreError;

#[async_trait]
pub trait ActiveLeaseStore: Send + Sync {
    async fn put(&self, record: &LeaseRecord) -> Result<(), ActiveLeaseStoreError>;
    async fn get(&self, lease_id: &LeaseId) -> Result<Option<LeaseRecord>, ActiveLeaseStoreError>;
    /// Removes the record. True only for the caller whose delete took effect,
    /// so concurrent callers cannot both claim the same lease for cleanup.
    async fn delete(&self, lease_id: &LeaseId) -> Result<bool, ActiveLeaseStoreError>;
    async fn all(&self) -> Result<Vec<LeaseRecord>, ActiveLeaseStoreError>;
    /// Records `at` as the lease's last activity without disturbing any other
    /// field a concurrent writer may be changing. Never moves the timestamp
    /// backwards. False when the record is gone.
    async fn touch(&self, lease_id: &LeaseId, at: SystemTime) -> Result<bool, ActiveLeaseStoreError>;
    /// Stores `record` only if the stored record still equals `expected`, so
    /// an out-of-band editor such as an operator tool cannot overwrite a
    /// change a broker made since it read the record. False when the record
    /// changed or is gone.
    async fn replace_if_unchanged(
        &self,
        expected: &LeaseRecord,
        record: &LeaseRecord,
    ) -> Result<bool, ActiveLeaseStoreError>;
}
