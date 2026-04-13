#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream;

use crate::{
    error::CronError,
    kv::{get_or_create_config_bucket, get_or_create_events_stream, get_or_create_snapshot_bucket},
    nats::validate_events_stream,
};

use super::catch_up_snapshots;

#[cfg(not(coverage))]
pub async fn connect_store(nats: async_nats::Client) -> Result<jetstream::Context, CronError> {
    let js = jetstream::new(nats);
    get_or_create_config_bucket(&js).await?;
    get_or_create_snapshot_bucket(&js).await?;
    validate_events_stream(&get_or_create_events_stream(&js).await?)?;
    catch_up_snapshots::run(&js).await?;
    Ok(js)
}

#[cfg(coverage)]
pub async fn connect_store(_nats: async_nats::Client) -> Result<jetstream::Context, CronError> {
    Err(CronError::event_source(
        "coverage stub does not provision the cron store",
        std::io::Error::other("coverage"),
    ))
}
