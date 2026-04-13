#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream;
use trogon_nats::jetstream::JetStreamGetStream;

use crate::{error::CronError, kv::EVENTS_STREAM};

#[cfg(not(coverage))]
pub(super) async fn run<J>(js: &J) -> Result<jetstream::stream::Stream, CronError>
where
    J: JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    js.get_stream(EVENTS_STREAM)
        .await
        .map_err(|source| CronError::event_source("failed to open events stream", source))
}

#[cfg(coverage)]
pub(super) async fn run<J>(_js: &J) -> Result<jetstream::stream::Stream, CronError>
where
    J: JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    Err(CronError::event_source(
        "coverage stub does not open the events stream",
        std::io::Error::other(EVENTS_STREAM),
    ))
}
