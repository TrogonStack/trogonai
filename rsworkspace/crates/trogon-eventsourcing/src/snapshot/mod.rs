mod read_snapshot_request;
mod read_snapshot_response;
mod snapshot_type;
#[path = "snapshot.rs"]
mod snapshot_value;
mod write_snapshot_request;
mod write_snapshot_response;

pub use read_snapshot_request::ReadSnapshotRequest;
pub use read_snapshot_response::ReadSnapshotResponse;
pub use snapshot_type::SnapshotType;
pub use snapshot_value::Snapshot;
pub use write_snapshot_request::WriteSnapshotRequest;
pub use write_snapshot_response::WriteSnapshotResponse;

pub trait SnapshotRead<SnapshotPayload, StreamId: ?Sized>: Send + Sync {
    type Error;

    fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, StreamId>,
    ) -> impl std::future::Future<Output = Result<ReadSnapshotResponse<SnapshotPayload>, Self::Error>> + Send;
}

pub trait SnapshotWrite<SnapshotPayload, StreamId: ?Sized>: Send + Sync {
    type Error;

    fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, SnapshotPayload, StreamId>,
    ) -> impl std::future::Future<Output = Result<WriteSnapshotResponse, Self::Error>> + Send;
}
