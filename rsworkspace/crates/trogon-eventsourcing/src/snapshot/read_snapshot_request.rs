#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadSnapshotRequest<'a, StreamId: ?Sized> {
    pub stream_id: &'a StreamId,
}
