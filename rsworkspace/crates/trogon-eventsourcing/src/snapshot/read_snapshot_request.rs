use super::SnapshotStoreConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadSnapshotRequest<'a, StreamId: ?Sized> {
    pub config: SnapshotStoreConfig,
    pub stream_id: &'a StreamId,
}

impl<'a, StreamId: ?Sized> ReadSnapshotRequest<'a, StreamId> {
    pub const fn new(config: SnapshotStoreConfig, stream_id: &'a StreamId) -> Self {
        Self { config, stream_id }
    }
}
