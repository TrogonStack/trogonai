use serde::{Serialize, de::DeserializeOwned};

use super::{JetStreamStore, JetStreamStoreError, StreamSubjectResolver};
use crate::snapshot::{ReadSnapshotRequest, ReadSnapshotResponse, WriteSnapshotRequest, WriteSnapshotResponse};
use crate::{SnapshotRead, SnapshotWrite};

impl<StreamId, Payload, Resolver> SnapshotRead<Payload, StreamId> for JetStreamStore<Resolver>
where
    StreamId: AsRef<str> + Send + Sync + ?Sized,
    Payload: Serialize + DeserializeOwned + Send,
    Resolver: StreamSubjectResolver<StreamId>,
{
    type Error = JetStreamStoreError<Resolver::Error>;

    async fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, StreamId>,
    ) -> Result<ReadSnapshotResponse<Payload>, Self::Error> {
        crate::nats::snapshot_store::read_snapshot(self.snapshot_bucket(), &request.config, request.stream_id.as_ref())
            .await
            .map(ReadSnapshotResponse::new)
            .map_err(JetStreamStoreError::Snapshot)
    }
}

impl<StreamId, Payload, Resolver> SnapshotWrite<Payload, StreamId> for JetStreamStore<Resolver>
where
    StreamId: AsRef<str> + Send + Sync + ?Sized,
    Payload: Serialize + DeserializeOwned + Send,
    Resolver: StreamSubjectResolver<StreamId>,
{
    type Error = JetStreamStoreError<Resolver::Error>;

    async fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, Payload, StreamId>,
    ) -> Result<WriteSnapshotResponse, Self::Error> {
        crate::nats::snapshot_store::write_snapshot(
            self.snapshot_bucket(),
            &request.config,
            request.stream_id.as_ref(),
            request.snapshot,
        )
        .await
        .map(|()| WriteSnapshotResponse::new())
        .map_err(JetStreamStoreError::Snapshot)
    }
}
