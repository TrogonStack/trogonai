use async_nats::jetstream;
use trogon_nats::jetstream::JetStreamGetStream;

use crate::{error::CronError, kv::EVENTS_STREAM};

pub(super) async fn run<J>(js: &J) -> Result<jetstream::stream::Stream, CronError>
where
    J: JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    js.get_stream(EVENTS_STREAM)
        .await
        .map_err(|source| CronError::event_source("failed to open events stream", source))
}
