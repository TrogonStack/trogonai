use super::Snapshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadSnapshotRequest<'a, StreamId: ?Sized> {
    pub stream_id: &'a StreamId,
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

pub trait SnapshotRead<SnapshotPayload, StreamId: ?Sized>: Send + Sync {
    type Error;

    fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, StreamId>,
    ) -> impl std::future::Future<Output = Result<ReadSnapshotResponse<SnapshotPayload>, Self::Error>> + Send;
}
