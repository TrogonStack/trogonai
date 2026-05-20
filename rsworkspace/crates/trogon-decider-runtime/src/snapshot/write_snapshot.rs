use super::Snapshot;

#[derive(Debug, Clone, PartialEq)]
pub struct WriteSnapshotRequest<'a, SnapshotPayload, StreamId: ?Sized> {
    pub stream_id: &'a StreamId,
    pub snapshot: Snapshot<SnapshotPayload>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriteSnapshotResponse;

pub trait SnapshotWrite<SnapshotPayload, StreamId: ?Sized>: Send + Sync {
    type Error;

    fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, SnapshotPayload, StreamId>,
    ) -> impl std::future::Future<Output = Result<WriteSnapshotResponse, Self::Error>> + Send;
}
