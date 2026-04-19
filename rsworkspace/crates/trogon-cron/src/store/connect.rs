#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream::{self, context, kv};
use serde::{Serialize, de::DeserializeOwned};
use trogon_eventsourcing::jetstream::{JetStreamStore, JetStreamStoreError};
use trogon_eventsourcing::{SnapshotStore, StreamAppend, StreamRead};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobId,
    error::CronError,
    kv::{
        get_or_create_cron_jobs_bucket, get_or_create_events_stream, get_or_create_snapshot_bucket,
    },
    nats::validate_events_stream,
    projections::{CronJobSnapshotProjector, catch_up_snapshots},
};

use super::GetCronJobsBucket;
use super::stream_subject::JobEventSubjectResolver;

type EventStore = JetStreamStore<JobEventSubjectResolver, CronJobSnapshotProjector>;

#[derive(Clone)]
pub struct Store {
    event_store: EventStore,
    cron_jobs_bucket: kv::Store,
}

impl Store {
    pub fn as_jetstream(&self) -> &jetstream::Context {
        self.event_store.as_jetstream()
    }

    pub fn command_runtime<Payload>(
        &self,
    ) -> &(
         impl StreamRead<JobId, Error = JetStreamStoreError<CronError>>
         + StreamAppend<JobId, Error = JetStreamStoreError<CronError>>
         + SnapshotStore<Payload, JobId, Error = JetStreamStoreError<CronError>>
         + '_
     )
    where
        Payload: Serialize + DeserializeOwned + Send,
    {
        &self.event_store
    }

    pub(crate) fn cron_jobs_bucket_ref(&self) -> &kv::Store {
        &self.cron_jobs_bucket
    }
}

impl GetCronJobsBucket for Store {
    type Store = kv::Store;

    fn cron_jobs_bucket(&self) -> &Self::Store {
        self.cron_jobs_bucket_ref()
    }
}

impl JetStreamGetKeyValue for Store {
    type Store = kv::Store;

    fn get_key_value<T: Into<String> + Send>(
        &self,
        bucket: T,
    ) -> impl std::future::Future<Output = Result<Self::Store, context::KeyValueError>> + Send {
        self.as_jetstream().get_key_value(bucket)
    }
}

impl JetStreamGetStream for Store {
    type Error = context::GetStreamError;
    type Stream = jetstream::stream::Stream;

    fn get_stream<T: AsRef<str> + Send>(
        &self,
        stream_name: T,
    ) -> impl std::future::Future<Output = Result<Self::Stream, Self::Error>> + Send {
        self.as_jetstream().get_stream(stream_name)
    }
}

impl JetStreamPublishMessage for Store {
    type PublishError = context::PublishError;
    type AckFuture = context::PublishAckFuture;

    fn publish_message(
        &self,
        message: async_nats::jetstream::message::OutboundMessage,
    ) -> impl std::future::Future<Output = Result<Self::AckFuture, Self::PublishError>> + Send {
        self.as_jetstream().publish_message(message)
    }
}

#[cfg(not(coverage))]
pub async fn connect_store(nats: async_nats::Client) -> Result<Store, CronError> {
    let js = jetstream::new(nats);
    let cron_jobs_bucket = get_or_create_cron_jobs_bucket(&js).await?;
    let snapshot_bucket = get_or_create_snapshot_bucket(&js).await?;
    let events_stream = get_or_create_events_stream(&js).await?;
    validate_events_stream(&events_stream)?;
    catch_up_snapshots(&js).await?;
    Ok(Store {
        event_store: JetStreamStore::new(
            js,
            events_stream,
            snapshot_bucket,
            JobEventSubjectResolver,
            CronJobSnapshotProjector,
        ),
        cron_jobs_bucket,
    })
}

#[cfg(coverage)]
pub async fn connect_store(_nats: async_nats::Client) -> Result<Store, CronError> {
    Err(CronError::event_source(
        "coverage stub does not provision the cron store",
        std::io::Error::other("coverage"),
    ))
}
