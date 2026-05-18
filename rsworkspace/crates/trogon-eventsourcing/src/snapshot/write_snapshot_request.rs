use super::{Snapshot, SnapshotStoreConfig};

#[derive(Debug, Clone, PartialEq)]
pub struct WriteSnapshotRequest<'a, SnapshotPayload, StreamId: ?Sized> {
    pub config: SnapshotStoreConfig,
    pub stream_id: &'a StreamId,
    pub snapshot: Snapshot<SnapshotPayload>,
}
