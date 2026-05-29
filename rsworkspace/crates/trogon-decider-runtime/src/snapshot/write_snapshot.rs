use super::Snapshot;

#[derive(Debug, Clone, PartialEq)]
pub struct WriteSnapshotRequest<'a, SnapshotPayload, SnapshotId: ?Sized> {
    pub snapshot_id: &'a SnapshotId,
    pub snapshot: Snapshot<SnapshotPayload>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriteSnapshotResponse;

pub trait SnapshotWrite<SnapshotPayload, SnapshotId: ?Sized>: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, SnapshotPayload, SnapshotId>,
    ) -> impl std::future::Future<Output = Result<WriteSnapshotResponse, Self::Error>> + Send;
}
