use super::Snapshot;

/// Request to load the most recent snapshot for one snapshot identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadSnapshotRequest<'a, SnapshotId: ?Sized> {
    /// Snapshot identity in the caller's domain-specific representation.
    pub snapshot_id: &'a SnapshotId,
}

/// Result of loading a snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadSnapshotResponse<SnapshotPayload> {
    /// The stored snapshot, or `None` if no snapshot exists yet for the
    /// requested identity.
    pub snapshot: Option<Snapshot<SnapshotPayload>>,
}

impl<SnapshotPayload> ReadSnapshotResponse<SnapshotPayload> {
    /// Extracts the loaded snapshot, discarding the response wrapper.
    pub fn into_snapshot(self) -> Option<Snapshot<SnapshotPayload>> {
        self.snapshot
    }
}

/// Loads snapshots for a decider state from a backing store.
pub trait SnapshotRead<SnapshotPayload, SnapshotId: ?Sized>: Send + Sync {
    /// Backend-specific read error.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Reads the most recent snapshot for the requested identity.
    fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, SnapshotId>,
    ) -> impl std::future::Future<Output = Result<ReadSnapshotResponse<SnapshotPayload>, Self::Error>> + Send;
}

#[cfg(test)]
mod tests;
