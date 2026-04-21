#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream::{self, kv};
use trogon_eventsourcing::nats::jetstream::JetStreamStore;

use crate::{
    error::CronError,
    kv::{
        get_or_create_cron_jobs_bucket, get_or_create_events_stream, get_or_create_snapshot_bucket,
    },
    nats::validate_events_stream,
    projections::{CronJobSnapshotProjector, catch_up_snapshots},
};

use super::stream_subject::JobEventSubjectResolver;

pub type EventStore = JetStreamStore<JobEventSubjectResolver, CronJobSnapshotProjector>;

#[derive(Clone)]
pub struct Store {
    pub event_store: EventStore,
    pub cron_jobs_bucket: kv::Store,
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
