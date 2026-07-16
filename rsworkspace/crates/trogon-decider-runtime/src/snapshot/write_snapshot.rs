use super::Snapshot;

/// Request to persist a snapshot for one snapshot identity.
#[derive(Debug, Clone, PartialEq)]
pub struct WriteSnapshotRequest<'a, SnapshotPayload, SnapshotId: ?Sized> {
    /// Snapshot identity in the caller's domain-specific representation.
    pub snapshot_id: &'a SnapshotId,
    /// The snapshot to persist.
    pub snapshot: Snapshot<SnapshotPayload>,
}

/// Result of a successful snapshot write. Carries no data; its existence
/// distinguishes success from the store's own error type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriteSnapshotResponse;

/// Persists snapshots for a decider state to a backing store.
pub trait SnapshotWrite<SnapshotPayload, SnapshotId: ?Sized>: Send + Sync {
    /// Backend-specific write error.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Persists the given snapshot for the requested identity.
    fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, SnapshotPayload, SnapshotId>,
    ) -> impl std::future::Future<Output = Result<WriteSnapshotResponse, Self::Error>> + Send;
}
