#![cfg_attr(coverage, allow(dead_code, unused_imports))]

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
    kv::{
        get_or_create_cron_jobs_bucket, get_or_create_events_stream, get_or_create_snapshot_bucket,
    },
    nats::validate_events_stream,
    projections::catch_up_snapshots,
};

use super::{
    append_events::run as append_job_events, append_events::stream_subject_state,
    events_stream::run as open_events_stream, snapshot_bucket::run as open_snapshot_bucket,
};

#[derive(Clone)]
pub struct Store {
    js: jetstream::Context,
}

impl Store {
    pub const fn as_jetstream(&self) -> &jetstream::Context {
        &self.js
    }
}

impl JetStreamGetKeyValue for Store {
    type Store = kv::Store;

    fn get_key_value<T: Into<String> + Send>(
        &self,
        bucket: T,
    ) -> impl std::future::Future<Output = Result<Self::Store, context::KeyValueError>> + Send {
        self.js.get_key_value(bucket)
    }
}

impl JetStreamGetStream for Store {
    type Error = context::GetStreamError;
    type Stream = jetstream::stream::Stream;

    fn get_stream<T: AsRef<str> + Send>(
        &self,
        stream_name: T,
    ) -> impl std::future::Future<Output = Result<Self::Stream, Self::Error>> + Send {
        self.js.get_stream(stream_name)
    }
}

impl JetStreamPublishMessage for Store {
    type PublishError = context::PublishError;
    type AckFuture = context::PublishAckFuture;

    fn publish_message(
        &self,
        message: async_nats::jetstream::message::OutboundMessage,
    ) -> impl std::future::Future<Output = Result<Self::AckFuture, Self::PublishError>> + Send {
        self.js.publish_message(message)
    }
}

impl EventStore<JobId> for Store {
    type Error = CronError;

    async fn current_stream_version(&self, stream_id: &JobId) -> Result<Option<u64>, Self::Error> {
        Ok(stream_subject_state(self, stream_id.as_str())
            .await?
            .write_state
            .current_version())
    }

    async fn read_stream_from(
        &self,
        stream_id: &JobId,
        from_sequence: u64,
    ) -> Result<Vec<RecordedEvent>, Self::Error> {
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

    async fn append_events(
        &self,
        stream_id: &JobId,
        expected_state: ExpectedState,
        events: NonEmpty<EventData>,
    ) -> Result<AppendOutcome, Self::Error> {
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

        append_job_events(
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

impl<Payload> SnapshotStore<Payload, JobId> for Store
where
    Payload: Serialize + DeserializeOwned + Send,
{
    type Error = CronError;

    async fn load_snapshot(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &JobId,
    ) -> Result<Option<Snapshot<Payload>>, Self::Error> {
        let bucket = open_snapshot_bucket(self).await?;
        load_snapshot(&bucket, config, stream_id.as_str())
            .await
            .map_err(CronError::from)
    }

    async fn save_snapshot(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &JobId,
        snapshot: Snapshot<Payload>,
    ) -> Result<(), Self::Error> {
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

#[cfg(not(coverage))]
pub async fn connect_store(nats: async_nats::Client) -> Result<Store, CronError> {
    let js = jetstream::new(nats);
    get_or_create_cron_jobs_bucket(&js).await?;
    get_or_create_snapshot_bucket(&js).await?;
    validate_events_stream(&get_or_create_events_stream(&js).await?)?;
    catch_up_snapshots(&js).await?;
    Ok(Store { js })
}

#[cfg(coverage)]
pub async fn connect_store(_nats: async_nats::Client) -> Result<Store, CronError> {
    Err(CronError::event_source(
        "coverage stub does not provision the cron store",
        std::io::Error::other("coverage"),
    ))
}
