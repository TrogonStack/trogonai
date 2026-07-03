#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream::{self, kv, stream};
use trogon_nats::jetstream::{
    JetStreamGetKeyValue, JetStreamGetStream, is_create_key_value_already_exists, is_create_stream_already_exists,
};

use crate::error::SchedulerError;

pub const EVENTS_STREAM: &str = "SCHEDULER_EVENTS";
pub const EVENTS_SUBJECT_PREFIX: &str = "scheduler.schedules.events.";
pub const EVENTS_SUBJECT_PATTERN: &str = "scheduler.schedules.events.>";
pub const COMMAND_SNAPSHOT_BUCKET: &str = "scheduler_command_snapshots";

// The schedules read-model bucket, its key scheme, and the catch-up checkpoint
// live with the projection that owns that storage layout; see
// `crate::projections::schedules::storage`. This module keeps only the shared
// NATS plumbing: the event stream (also used by the event store), the command
// snapshot bucket, and the generic create-or-open helper.

#[cfg(not(coverage))]
pub async fn get_or_create_command_snapshot_bucket(js: &jetstream::Context) -> Result<kv::Store, SchedulerError> {
    get_or_create(
        js,
        kv::Config {
            bucket: COMMAND_SNAPSHOT_BUCKET.to_string(),
            history: 1,
            ..Default::default()
        },
    )
    .await
}

#[cfg(not(coverage))]
pub async fn get_or_create(js: &jetstream::Context, config: kv::Config) -> Result<kv::Store, SchedulerError> {
    let name = config.bucket.clone();
    match js.create_key_value(config).await {
        Ok(store) => Ok(store),
        Err(source) if is_create_key_value_already_exists(&source) => js.get_key_value(&name).await.map_err(|source| {
            SchedulerError::kv_source(
                "failed to get existing key-value bucket after create reported already exists",
                source,
            )
        }),
        Err(source) => Err(SchedulerError::kv_source("failed to create key-value bucket", source)),
    }
}

#[cfg(not(coverage))]
pub async fn get_or_create_events_stream(js: &jetstream::Context) -> Result<stream::Stream, SchedulerError> {
    let config = stream::Config {
        name: EVENTS_STREAM.to_string(),
        subjects: vec![EVENTS_SUBJECT_PATTERN.to_string()],
        allow_atomic_publish: true,
        // The read model is rebuilt by folding the full event history, so the stream
        // must retain everything. Set the retention limits explicitly to the values
        // `validate_events_stream` requires instead of relying on the server to
        // normalize `Config::default()`'s `0` limits to unlimited.
        retention: stream::RetentionPolicy::Limits,
        max_age: std::time::Duration::ZERO,
        max_messages: -1,
        max_messages_per_subject: -1,
        max_bytes: -1,
        ..Default::default()
    };

    let stream = match js.create_stream(config.clone()).await {
        Ok(stream) => stream,
        Err(source) if is_create_stream_already_exists(&source) => {
            js.get_stream(EVENTS_STREAM).await.map_err(|source| {
                SchedulerError::event_source(
                    "failed to get existing events stream after create reported already exists",
                    source,
                )
            })?
        }
        Err(source) => {
            return Err(SchedulerError::event_source(
                "failed to get or create events stream",
                source,
            ));
        }
    };

    ensure_events_stream_config(js, stream, config).await
}

#[cfg(not(coverage))]
async fn ensure_events_stream_config(
    js: &jetstream::Context,
    stream: stream::Stream,
    desired: stream::Config,
) -> Result<stream::Stream, SchedulerError> {
    let mut current = stream.cached_info().config.clone();
    if current.allow_atomic_publish == desired.allow_atomic_publish && current.subjects == desired.subjects {
        return Ok(stream);
    }

    current.allow_atomic_publish = desired.allow_atomic_publish;
    current.subjects = desired.subjects;
    js.update_stream(current)
        .await
        .map_err(|source| SchedulerError::event_source("failed to update events stream configuration", source))?;
    js.get_stream(EVENTS_STREAM)
        .await
        .map_err(|source| SchedulerError::event_source("failed to reopen updated events stream", source))
}

#[cfg(not(coverage))]
pub async fn open_command_snapshot_bucket<J>(js: &J) -> Result<kv::Store, SchedulerError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    js.get_key_value(COMMAND_SNAPSHOT_BUCKET)
        .await
        .map_err(|source| SchedulerError::kv_source("failed to open command snapshot bucket", source))
}

#[cfg(not(coverage))]
pub(crate) async fn open_events_stream<J>(js: &J) -> Result<stream::Stream, SchedulerError>
where
    J: JetStreamGetStream<Stream = stream::Stream>,
{
    js.get_stream(EVENTS_STREAM)
        .await
        .map_err(|source| SchedulerError::event_source("failed to open events stream", source))
}

#[cfg(test)]
mod tests;
