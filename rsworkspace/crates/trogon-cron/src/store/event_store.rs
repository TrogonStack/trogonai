use async_nats::jetstream::{self, kv};
use serde::{Serialize, de::DeserializeOwned};
use trogon_eventsourcing::nats::jetstream::JetStreamStore;
use trogon_eventsourcing::{
    AppendStreamRequest, AppendStreamResponse, ReadSnapshotRequest, ReadSnapshotResponse, ReadStreamRequest,
    ReadStreamResponse, SnapshotRead, SnapshotWrite, StreamAppend, StreamRead, WriteSnapshotRequest,
    WriteSnapshotResponse,
};

use super::stream_subject::JobEventSubjectResolver;
use crate::{error::CronError, projections::project_appended_events};

#[derive(Clone)]
pub struct EventStore {
    inner: JetStreamStore<JobEventSubjectResolver>,
}

impl EventStore {
    pub fn new(js: jetstream::Context, events_stream: jetstream::stream::Stream, snapshot_bucket: kv::Store) -> Self {
        Self {
            inner: JetStreamStore::new(js, events_stream, snapshot_bucket, JobEventSubjectResolver),
        }
    }

    pub fn events_stream(&self) -> &jetstream::stream::Stream {
        self.inner.events_stream()
    }
}

impl StreamRead<str> for EventStore {
    type Error = CronError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        self.inner.read_stream(request).await.map_err(CronError::from)
    }
}

impl StreamAppend<str> for EventStore {
    type Error = CronError;

    async fn append_stream(&self, request: AppendStreamRequest<'_, str>) -> Result<AppendStreamResponse, Self::Error> {
        let stream_id = request.stream_id;
        let projected_events = request.events.as_slice().to_vec();
        let outcome = self.inner.append_stream(request).await.map_err(CronError::from)?;

        project_appended_events(
            self.inner.snapshot_bucket(),
            stream_id,
            projected_events.as_slice(),
            outcome.next_expected_version,
        )
        .await?;

        Ok(outcome)
    }
}

impl<Payload> SnapshotRead<Payload, str> for EventStore
where
    Payload: Serialize + DeserializeOwned + Send,
{
    type Error = CronError;

    async fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, str>,
    ) -> Result<ReadSnapshotResponse<Payload>, Self::Error> {
        self.inner.read_snapshot(request).await.map_err(CronError::from)
    }
}

impl<Payload> SnapshotWrite<Payload, str> for EventStore
where
    Payload: Serialize + DeserializeOwned + Send,
{
    type Error = CronError;

    async fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, Payload, str>,
    ) -> Result<WriteSnapshotResponse, Self::Error> {
        self.inner.write_snapshot(request).await.map_err(CronError::from)
    }
}
