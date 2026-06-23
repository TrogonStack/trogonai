use super::Snapshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadSnapshotRequest<'a, SnapshotId: ?Sized> {
    pub snapshot_id: &'a SnapshotId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadSnapshotResponse<SnapshotPayload> {
    pub snapshot: Option<Snapshot<SnapshotPayload>>,
}

impl<SnapshotPayload> ReadSnapshotResponse<SnapshotPayload> {
    pub fn into_snapshot(self) -> Option<Snapshot<SnapshotPayload>> {
        self.snapshot
    }
}

pub trait SnapshotRead<SnapshotPayload, SnapshotId: ?Sized>: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, SnapshotId>,
    ) -> impl std::future::Future<Output = Result<ReadSnapshotResponse<SnapshotPayload>, Self::Error>> + Send;
}

#[cfg(test)]
mod tests;
