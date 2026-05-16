use super::{Snapshot, SnapshotStoreConfig};

#[derive(Debug, Clone, PartialEq)]
pub struct WriteSnapshotRequest<'a, SnapshotPayload, StreamId: ?Sized> {
    pub config: SnapshotStoreConfig,
    pub stream_id: &'a StreamId,
    pub snapshot: Snapshot<SnapshotPayload>,
}

impl<'a, SnapshotPayload, StreamId: ?Sized> WriteSnapshotRequest<'a, SnapshotPayload, StreamId> {
    pub const fn new(
        config: SnapshotStoreConfig,
        stream_id: &'a StreamId,
        snapshot: Snapshot<SnapshotPayload>,
    ) -> Self {
        Self {
            config,
            stream_id,
            snapshot,
        }
    }
}
