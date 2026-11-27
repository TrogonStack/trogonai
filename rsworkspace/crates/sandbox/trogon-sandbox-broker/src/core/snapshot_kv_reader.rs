use super::*;

/// Read-only reader over the [`SnapshotKvRecord`]s persisted by
/// `sandbox-snapshot-builder`'s `SnapshotKvStore`. This crate does not
/// depend on that crate, so a reader talks directly to the same
/// `SNAPSHOT_KV_BUCKET` JetStream KV bucket, following the same structural
/// pattern as [`DriveStore`]/`NatsKvDriveStore`/[`InMemoryDriveStore`].
#[async_trait::async_trait]
pub trait SnapshotKvReader: Send + Sync {
    async fn get(&self, digest: &Digest) -> Result<Option<SnapshotKvRecord>, SnapshotKvError>;
    async fn get_owned(&self, tenant: &TenantId, digest: &Digest) -> Result<Option<SnapshotKvRecord>, SnapshotKvError> {
        Ok(self.get(digest).await?.filter(|record| record.tenant == *tenant))
    }
    async fn record(&self, _record: &SnapshotKvRecord) -> Result<(), SnapshotKvError> {
        Err(SnapshotKvError::WritesUnavailable)
    }
}
