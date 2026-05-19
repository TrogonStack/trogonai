use super::Snapshot;

#[derive(Debug, Clone, PartialEq)]
pub struct WriteSnapshotRequest<'a, SnapshotPayload, StreamId: ?Sized> {
    pub stream_id: &'a StreamId,
    pub snapshot: Snapshot<SnapshotPayload>,
}
