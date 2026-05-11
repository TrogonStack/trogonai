use super::{ReadSnapshotRequest, ReadSnapshotResponse};

pub trait SnapshotRead<SnapshotPayload, StreamId: ?Sized>: Send + Sync {
    type Error;

    fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, StreamId>,
    ) -> impl std::future::Future<Output = Result<ReadSnapshotResponse<SnapshotPayload>, Self::Error>> + Send;
}
