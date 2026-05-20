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
    type Error: std::error::Error + Send + Sync + 'static;

    fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, StreamId>,
    ) -> impl std::future::Future<Output = Result<ReadSnapshotResponse<SnapshotPayload>, Self::Error>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StreamPosition;

    #[test]
    fn response_returns_loaded_snapshot() {
        let position = StreamPosition::try_new(7).unwrap();
        let snapshot = Snapshot::new(position, "payload");
        let response = ReadSnapshotResponse {
            snapshot: Some(snapshot.clone()),
        };

        assert_eq!(response.into_snapshot(), Some(snapshot));
    }
}
