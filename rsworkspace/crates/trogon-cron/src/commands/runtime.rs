use async_nats::jetstream::{self, context, kv};
use serde::{Serialize, de::DeserializeOwned};
use trogon_eventsourcing::{
    AppendOutcome, EventData, ExecutionRuntime, ExpectedVersion, NonEmpty, RecordedEvent, Snapshot,
    SnapshotChange, SnapshotStoreConfig, load_snapshot, persist_snapshot_change, read_stream_from,
};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobId, JobWriteCondition,
    error::CronError,
    store::{append_events, open_events_stream, open_snapshot_bucket, stream_subject_state},
};

pub trait CronCommandExecutionRuntime<Payload>: Send + Sync {
    fn load_command_snapshot(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &JobId,
    ) -> impl std::future::Future<Output = Result<Option<Snapshot<Payload>>, CronError>> + Send;

    fn save_command_snapshot(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &JobId,
        snapshot: Snapshot<Payload>,
    ) -> impl std::future::Future<Output = Result<(), CronError>> + Send;

    fn current_job_stream_version(
        &self,
        stream_id: &JobId,
    ) -> impl std::future::Future<Output = Result<Option<u64>, CronError>> + Send;

    fn read_job_stream_from(
        &self,
        stream_id: &JobId,
        from_sequence: u64,
    ) -> impl std::future::Future<Output = Result<Vec<RecordedEvent>, CronError>> + Send;

    fn append_job_events(
        &self,
        stream_id: &JobId,
        expected_version: ExpectedVersion,
        events: NonEmpty<EventData>,
    ) -> impl std::future::Future<Output = Result<AppendOutcome, CronError>> + Send;
}

pub(crate) struct CronCommandRuntime<'a, R> {
    runtime: &'a R,
    snapshot_config: SnapshotStoreConfig<'static>,
}

impl<'a, R> CronCommandRuntime<'a, R> {
    pub(crate) const fn new(runtime: &'a R, snapshot_config: SnapshotStoreConfig<'static>) -> Self {
        Self {
            runtime,
            snapshot_config,
        }
    }
}

impl<Payload, R> ExecutionRuntime<Payload, JobId> for CronCommandRuntime<'_, R>
where
    R: CronCommandExecutionRuntime<Payload>,
    Payload: Send,
{
    type Error = CronError;

    async fn load_snapshot(
        &self,
        stream_id: &JobId,
    ) -> Result<Option<Snapshot<Payload>>, Self::Error> {
        self.runtime
            .load_command_snapshot(self.snapshot_config, stream_id)
            .await
    }

    async fn save_snapshot(
        &self,
        stream_id: &JobId,
        snapshot: Snapshot<Payload>,
    ) -> Result<(), Self::Error> {
        self.runtime
            .save_command_snapshot(self.snapshot_config, stream_id, snapshot)
            .await
    }

    async fn current_stream_version(&self, stream_id: &JobId) -> Result<Option<u64>, Self::Error> {
        self.runtime.current_job_stream_version(stream_id).await
    }

    async fn read_stream_from(
        &self,
        stream_id: &JobId,
        from_sequence: u64,
    ) -> Result<Vec<RecordedEvent>, Self::Error> {
        self.runtime
            .read_job_stream_from(stream_id, from_sequence)
            .await
    }

    async fn append_events(
        &self,
        stream_id: &JobId,
        expected_version: ExpectedVersion,
        events: NonEmpty<EventData>,
    ) -> Result<AppendOutcome, Self::Error> {
        self.runtime
            .append_job_events(stream_id, expected_version, events)
            .await
    }
}

fn write_condition_from_expected_version(expected_version: ExpectedVersion) -> JobWriteCondition {
    match expected_version {
        ExpectedVersion::NoStream => JobWriteCondition::MustNotExist,
        ExpectedVersion::StreamVersion(version) => JobWriteCondition::MustBeAtVersion(version),
    }
}

impl<Payload, J> CronCommandExecutionRuntime<Payload> for J
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        > + Send
        + Sync,
    Payload: Serialize + DeserializeOwned + Send,
{
    async fn load_command_snapshot(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &JobId,
    ) -> Result<Option<Snapshot<Payload>>, CronError> {
        let bucket = open_snapshot_bucket(self).await?;
        load_snapshot(&bucket, config, stream_id.as_str())
            .await
            .map_err(CronError::from)
    }

    async fn save_command_snapshot(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &JobId,
        snapshot: Snapshot<Payload>,
    ) -> Result<(), CronError> {
        let bucket = open_snapshot_bucket(self).await?;
        persist_snapshot_change(
            &bucket,
            config,
            SnapshotChange::upsert(stream_id.as_str(), snapshot),
        )
        .await
        .map_err(CronError::from)
    }

    async fn current_job_stream_version(
        &self,
        stream_id: &JobId,
    ) -> Result<Option<u64>, CronError> {
        Ok(stream_subject_state(self, stream_id.as_str())
            .await?
            .write_state
            .current_version())
    }

    async fn read_job_stream_from(
        &self,
        stream_id: &JobId,
        from_sequence: u64,
    ) -> Result<Vec<RecordedEvent>, CronError> {
        let stream = open_events_stream(self).await?;
        read_stream_from(&stream, from_sequence)
            .await
            .map_err(|source| {
                CronError::event_source(
                    "failed to read job stream while catching up command state",
                    source,
                )
            })
            .map(|events| {
                events
                    .into_iter()
                    .filter(|event| event.stream_id() == stream_id.as_str())
                    .collect()
            })
    }

    async fn append_job_events(
        &self,
        stream_id: &JobId,
        expected_version: ExpectedVersion,
        events: NonEmpty<EventData>,
    ) -> Result<AppendOutcome, CronError> {
        let current_version = stream_subject_state(self, stream_id.as_str())
            .await?
            .write_state
            .current_version()
            .unwrap_or(0);
        let appended_events = events.len() as u64;
        append_events(
            self,
            stream_id.as_str(),
            write_condition_from_expected_version(expected_version),
            events,
        )
        .await?;
        Ok(AppendOutcome {
            next_expected_version: current_version + appended_events,
        })
    }
}
