#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream::{self, kv};

use crate::{
    error::CronError,
    kv::{get_or_create_command_snapshot_bucket, get_or_create_cron_jobs_bucket, get_or_create_events_stream},
    nats::validate_events_stream,
    projections::catch_up_cron_jobs_read_model,
};

use super::event_store::EventStore;

#[derive(Clone)]
pub struct Store {
    pub event_store: EventStore,
    pub cron_jobs_bucket: kv::Store,
}

#[cfg(not(coverage))]
pub async fn connect_store(nats: async_nats::Client) -> Result<Store, CronError> {
    let js = jetstream::new(nats);
    let cron_jobs_bucket = get_or_create_cron_jobs_bucket(&js).await?;
    let command_snapshot_bucket = get_or_create_command_snapshot_bucket(&js).await?;
    let events_stream = get_or_create_events_stream(&js).await?;
    validate_events_stream(&events_stream)?;
    catch_up_cron_jobs_read_model(&js).await?;
    Ok(Store {
        event_store: EventStore::new(js, events_stream, command_snapshot_bucket, cron_jobs_bucket.clone()),
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
