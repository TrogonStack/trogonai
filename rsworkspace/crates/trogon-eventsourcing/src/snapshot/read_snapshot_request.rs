use super::SnapshotStoreConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadSnapshotRequest<'a, StreamId: ?Sized> {
    pub config: SnapshotStoreConfig,
    pub stream_id: &'a StreamId,
}
