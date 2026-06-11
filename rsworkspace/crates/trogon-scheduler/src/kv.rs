#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream::{self, kv, stream};
use trogon_nats::jetstream::{
    JetStreamGetKeyValue, JetStreamGetStream, is_create_key_value_already_exists, is_create_stream_already_exists,
};

use crate::error::SchedulerError;

pub const SCHEDULES_BUCKET: &str = "scheduler_schedules";
pub const EVENTS_STREAM: &str = "SCHEDULER_EVENTS";
pub const EVENTS_SUBJECT_PREFIX: &str = "scheduler.schedules.events.";
pub const EVENTS_SUBJECT_PATTERN: &str = "scheduler.schedules.events.>";
pub const COMMAND_SNAPSHOT_BUCKET: &str = "scheduler_command_snapshots";
pub const SCHEDULES_CHECKPOINT_KEY: &str = "_query.schedules.last_event_sequence";

#[cfg(not(coverage))]
pub async fn get_or_create_schedules_bucket(js: &jetstream::Context) -> Result<kv::Store, SchedulerError> {
    get_or_create(
        js,
        kv::Config {
            bucket: SCHEDULES_BUCKET.to_string(),
            history: 5,
            ..Default::default()
        },
    )
    .await
}

#[cfg(coverage)]
pub async fn get_or_create_schedules_bucket(_js: &jetstream::Context) -> Result<kv::Store, SchedulerError> {
    Err(SchedulerError::kv_source(
        "coverage stub does not provision schedules buckets",
        std::io::Error::other(SCHEDULES_BUCKET),
    ))
}

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

#[cfg(coverage)]
pub async fn get_or_create_command_snapshot_bucket(_js: &jetstream::Context) -> Result<kv::Store, SchedulerError> {
    Err(SchedulerError::kv_source(
        "coverage stub does not provision command snapshot buckets",
        std::io::Error::other(COMMAND_SNAPSHOT_BUCKET),
    ))
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

#[cfg(coverage)]
pub async fn get_or_create(_js: &jetstream::Context, _config: kv::Config) -> Result<kv::Store, SchedulerError> {
    Err(SchedulerError::kv_source(
        "coverage stub does not provision key-value buckets",
        std::io::Error::other("coverage"),
    ))
}

#[cfg(not(coverage))]
pub async fn get_or_create_events_stream(js: &jetstream::Context) -> Result<stream::Stream, SchedulerError> {
    let config = stream::Config {
        name: EVENTS_STREAM.to_string(),
        subjects: vec![EVENTS_SUBJECT_PATTERN.to_string()],
        allow_atomic_publish: true,
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

#[cfg(coverage)]
pub async fn get_or_create_events_stream(_js: &jetstream::Context) -> Result<stream::Stream, SchedulerError> {
    Err(SchedulerError::event_source(
        "coverage stub does not provision events streams",
        std::io::Error::other(EVENTS_STREAM),
    ))
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

#[cfg(coverage)]
pub async fn open_command_snapshot_bucket<J>(_js: &J) -> Result<kv::Store, SchedulerError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    Err(SchedulerError::kv_source(
        "coverage stub does not open the command snapshot bucket",
        std::io::Error::other(COMMAND_SNAPSHOT_BUCKET),
    ))
}

#[cfg(not(coverage))]
pub(crate) async fn open_schedules_bucket<J>(js: &J) -> Result<kv::Store, SchedulerError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    js.get_key_value(SCHEDULES_BUCKET)
        .await
        .map_err(|source| SchedulerError::kv_source("failed to open schedules bucket", source))
}

#[cfg(coverage)]
pub(crate) async fn open_schedules_bucket<J>(_js: &J) -> Result<kv::Store, SchedulerError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    Err(SchedulerError::kv_source(
        "coverage stub does not open the schedules bucket",
        std::io::Error::other(SCHEDULES_BUCKET),
    ))
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

#[cfg(coverage)]
pub(crate) async fn open_events_stream<J>(_js: &J) -> Result<stream::Stream, SchedulerError>
where
    J: JetStreamGetStream<Stream = stream::Stream>,
{
    Err(SchedulerError::event_source(
        "coverage stub does not open the events stream",
        std::io::Error::other(EVENTS_STREAM),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_nats::jetstream::context::{
        CreateKeyValueError, CreateKeyValueErrorKind, CreateStreamError, CreateStreamErrorKind,
    };

    fn stream_exists_error() -> CreateStreamError {
        let source: jetstream::Error = serde_json::from_str(
            r#"{"code":400,"err_code":10058,"description":"stream name already in use with a different configuration"}"#,
        )
        .unwrap();

        CreateStreamError::new(CreateStreamErrorKind::JetStream(source))
    }

    #[test]
    fn create_key_value_already_exists_matches_wrapped_stream_exists_error() {
        let error = CreateKeyValueError::with_source(CreateKeyValueErrorKind::BucketCreate, stream_exists_error());

        assert!(is_create_key_value_already_exists(&error));
    }
}
