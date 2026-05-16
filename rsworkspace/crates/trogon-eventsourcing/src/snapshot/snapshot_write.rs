use super::{WriteSnapshotRequest, WriteSnapshotResponse};

pub trait SnapshotWrite<SnapshotPayload, StreamId: ?Sized>: Send + Sync {
    type Error;

    fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, SnapshotPayload, StreamId>,
    ) -> impl std::future::Future<Output = Result<WriteSnapshotResponse, Self::Error>> + Send;
}
