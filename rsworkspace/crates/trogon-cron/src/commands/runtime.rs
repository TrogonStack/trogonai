use async_nats::jetstream::{self, context, kv};
use serde::{Serialize, de::DeserializeOwned};
use trogon_eventsourcing::{
    AppendOutcome, EventData, ExecutionRuntime, ExpectedState, NonEmpty, RecordedEvent, Snapshot,
    SnapshotChange, SnapshotRuntime, SnapshotStoreConfig, load_snapshot, persist_snapshot_change,
    read_stream_from,
};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobId, JobWriteCondition,
    error::CronError,
    store::{append_events, open_events_stream, open_snapshot_bucket, stream_subject_state},
};

pub trait CronCommandRuntimePort: Send + Sync {
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
        expected_state: ExpectedState,
        events: NonEmpty<EventData>,
    ) -> impl std::future::Future<Output = Result<AppendOutcome, CronError>> + Send;
}

pub trait CronCommandSnapshotRuntime<Payload>: Send + Sync {
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

impl<R> ExecutionRuntime<JobId> for CronCommandRuntime<'_, R>
where
    R: CronCommandRuntimePort,
{
    type Error = CronError;

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
        expected_state: ExpectedState,
        events: NonEmpty<EventData>,
    ) -> Result<AppendOutcome, Self::Error> {
        self.runtime
            .append_job_events(stream_id, expected_state, events)
            .await
    }
}

impl<Payload, R> SnapshotRuntime<Payload, JobId> for CronCommandRuntime<'_, R>
where
    R: CronCommandSnapshotRuntime<Payload>,
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
}

impl<J> CronCommandRuntimePort for J
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        > + Send
        + Sync,
{
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
        expected_state: ExpectedState,
        events: NonEmpty<EventData>,
    ) -> Result<AppendOutcome, CronError> {
        let stream_state = stream_subject_state(self, stream_id.as_str()).await?;
        let current_version = stream_state.write_state.current_version();
        let appended_events = events.len() as u64;
        let write_condition = match expected_state {
            ExpectedState::Any => None,
            ExpectedState::StreamExists => Some(
                current_version
                    .map(JobWriteCondition::MustBeAtVersion)
                    .ok_or_else(|| CronError::OptimisticConcurrencyConflict {
                        id: stream_id.to_string(),
                        expected: ExpectedState::StreamExists,
                        current_version,
                    })?,
            ),
            ExpectedState::NoStream => Some(JobWriteCondition::MustNotExist),
            ExpectedState::StreamRevision(version) => {
                Some(JobWriteCondition::MustBeAtVersion(version))
            }
        };
        append_events(
            self,
            stream_id.as_str(),
            write_condition.unwrap_or(JobWriteCondition::MustBeAtVersion(
                current_version.unwrap_or(0),
            )),
            events,
        )
        .await?;
        Ok(AppendOutcome {
            next_expected_version: current_version.unwrap_or(0) + appended_events,
        })
    }
}

impl<Payload, J> CronCommandSnapshotRuntime<Payload> for J
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
}
