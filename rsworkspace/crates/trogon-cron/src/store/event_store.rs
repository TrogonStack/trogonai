use async_nats::jetstream::{self, kv};
use serde::{Serialize, de::DeserializeOwned};
use trogon_eventsourcing::nats::jetstream::JetStreamStore;
use trogon_eventsourcing::{
    AppendOutcome, EventData, NonEmpty, Snapshot, SnapshotRead, SnapshotStoreConfig, SnapshotWrite, StreamAppend,
    StreamRead, StreamReadResult, StreamState,
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

    async fn read_stream_from(&self, stream_id: &str, from_sequence: u64) -> Result<StreamReadResult, Self::Error> {
        self.inner
            .read_stream_from(stream_id, from_sequence)
            .await
            .map_err(CronError::from)
    }
}

impl StreamAppend<str> for EventStore {
    type Error = CronError;

    async fn append_events(
        &self,
        stream_id: &str,
        stream_state: StreamState,
        events: NonEmpty<EventData>,
    ) -> Result<AppendOutcome, Self::Error> {
        let projected_events = events.as_slice().to_vec();
        let outcome = self
            .inner
            .append_events(stream_id, stream_state, events)
            .await
            .map_err(CronError::from)?;

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

    async fn load_snapshot(
        &self,
        config: SnapshotStoreConfig,
        stream_id: &str,
    ) -> Result<Option<Snapshot<Payload>>, Self::Error> {
        self.inner
            .load_snapshot(config, stream_id)
            .await
            .map_err(CronError::from)
    }
}

impl<Payload> SnapshotWrite<Payload, str> for EventStore
where
    Payload: Serialize + DeserializeOwned + Send,
{
    type Error = CronError;

    async fn save_snapshot(
        &self,
        config: SnapshotStoreConfig,
        stream_id: &str,
        snapshot: Snapshot<Payload>,
    ) -> Result<(), Self::Error> {
        self.inner
            .save_snapshot(config, stream_id, snapshot)
            .await
            .map_err(CronError::from)
    }
}
