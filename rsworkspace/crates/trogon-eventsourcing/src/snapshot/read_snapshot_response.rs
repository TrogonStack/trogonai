use super::Snapshot;

#[derive(Debug, Clone, PartialEq)]
pub struct ReadSnapshotResponse<SnapshotPayload> {
    pub snapshot: Option<Snapshot<SnapshotPayload>>,
}

impl<SnapshotPayload> ReadSnapshotResponse<SnapshotPayload> {
    pub const fn new(snapshot: Option<Snapshot<SnapshotPayload>>) -> Self {
        Self { snapshot }
    }

    pub fn into_snapshot(self) -> Option<Snapshot<SnapshotPayload>> {
        self.snapshot
    }
}
