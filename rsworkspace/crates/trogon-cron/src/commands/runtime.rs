use async_nats::jetstream::{self, context, kv};
use serde::{Serialize, de::DeserializeOwned};
use trogon_eventsourcing::{
    AppendOutcome, EventData, EventStore, ExpectedState, NonEmpty, RecordedEvent, Snapshot,
    SnapshotChange, SnapshotStore, SnapshotStoreConfig, load_snapshot, persist_snapshot_change,
    read_stream_from,
};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobId, JobWriteCondition,
    error::CronError,
    store::{append_events, open_events_stream, open_snapshot_bucket, stream_subject_state},
};

#[doc(hidden)]
pub trait CommandRuntime: Send + Sync {
    fn current_command_stream_version(
        &self,
        stream_id: &JobId,
    ) -> impl std::future::Future<Output = Result<Option<u64>, CronError>> + Send;

    fn read_command_stream_from(
        &self,
        stream_id: &JobId,
        from_sequence: u64,
    ) -> impl std::future::Future<Output = Result<Vec<RecordedEvent>, CronError>> + Send;

    fn append_command_events(
        &self,
        stream_id: &JobId,
        expected_state: ExpectedState,
        events: NonEmpty<EventData>,
    ) -> impl std::future::Future<Output = Result<AppendOutcome, CronError>> + Send;

    fn load_command_snapshot<Payload>(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &JobId,
    ) -> impl std::future::Future<Output = Result<Option<Snapshot<Payload>>, CronError>> + Send
    where
        Payload: DeserializeOwned + Send;

    fn save_command_snapshot<Payload>(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &JobId,
        snapshot: Snapshot<Payload>,
    ) -> impl std::future::Future<Output = Result<(), CronError>> + Send
    where
        Payload: Serialize + Send;
}

struct EventStoreAdapter<'a, R> {
    runtime: &'a R,
}

struct SnapshotStoreAdapter<'a, R> {
    runtime: &'a R,
}

pub(crate) fn event_store<R>(runtime: &R) -> impl EventStore<JobId, Error = CronError> + use<'_, R>
where
    R: CommandRuntime,
{
    EventStoreAdapter { runtime }
}

pub(crate) fn snapshot_store<R, Payload>(
    runtime: &R,
) -> impl SnapshotStore<Payload, JobId, Error = CronError> + use<'_, R, Payload>
where
    R: CommandRuntime,
    Payload: Serialize + DeserializeOwned + Send,
{
    SnapshotStoreAdapter { runtime }
}

impl<R> EventStore<JobId> for EventStoreAdapter<'_, R>
where
    R: CommandRuntime,
{
    type Error = CronError;

    async fn current_stream_version(&self, stream_id: &JobId) -> Result<Option<u64>, Self::Error> {
        self.runtime.current_command_stream_version(stream_id).await
    }

    async fn read_stream_from(
        &self,
        stream_id: &JobId,
        from_sequence: u64,
    ) -> Result<Vec<RecordedEvent>, Self::Error> {
        self.runtime
            .read_command_stream_from(stream_id, from_sequence)
            .await
    }

    async fn append_events(
        &self,
        stream_id: &JobId,
        expected_state: ExpectedState,
        events: NonEmpty<EventData>,
    ) -> Result<AppendOutcome, Self::Error> {
        self.runtime
            .append_command_events(stream_id, expected_state, events)
            .await
    }
}

impl<Payload, R> SnapshotStore<Payload, JobId> for SnapshotStoreAdapter<'_, R>
where
    R: CommandRuntime,
    Payload: Serialize + DeserializeOwned + Send,
{
    type Error = CronError;

    async fn load_snapshot(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &JobId,
    ) -> Result<Option<Snapshot<Payload>>, Self::Error> {
        self.runtime.load_command_snapshot(config, stream_id).await
    }

    async fn save_snapshot(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &JobId,
        snapshot: Snapshot<Payload>,
    ) -> Result<(), Self::Error> {
        self.runtime
            .save_command_snapshot(config, stream_id, snapshot)
            .await
    }
}

impl<J> CommandRuntime for J
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        > + Send
        + Sync,
{
    async fn current_command_stream_version(
        &self,
        stream_id: &JobId,
    ) -> Result<Option<u64>, CronError> {
        Ok(stream_subject_state(self, stream_id.as_str())
            .await?
            .write_state
            .current_version())
    }

    async fn read_command_stream_from(
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

    async fn append_command_events(
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

    async fn load_command_snapshot<Payload>(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &JobId,
    ) -> Result<Option<Snapshot<Payload>>, CronError>
    where
        Payload: DeserializeOwned + Send,
    {
        let bucket = open_snapshot_bucket(self).await?;
        load_snapshot(&bucket, config, stream_id.as_str())
            .await
            .map_err(CronError::from)
    }

    async fn save_command_snapshot<Payload>(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &JobId,
        snapshot: Snapshot<Payload>,
    ) -> Result<(), CronError>
    where
        Payload: Serialize + Send,
    {
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
