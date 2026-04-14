#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream::{self, kv, stream};
use trogon_nats::jetstream::{is_create_key_value_already_exists, is_create_stream_already_exists};

use crate::error::CronError;

pub const CRON_JOBS_BUCKET: &str = "cron_jobs";
pub const LEADER_BUCKET: &str = "cron_leader";
pub const LEADER_KEY: &str = "lock";
pub const EVENTS_STREAM: &str = "CRON_EVENTS";
pub const EVENTS_SUBJECT_PREFIX: &str = "cron.jobs.events.";
pub const EVENTS_SUBJECT_PATTERN: &str = "cron.jobs.events.>";
pub const LEGACY_EVENTS_SUBJECT_PREFIX: &str = "cron.events.jobs.";
pub const LEGACY_EVENTS_SUBJECT_PATTERN: &str = "cron.events.jobs.>";
pub const SNAPSHOT_BUCKET: &str = "cron_snapshots";
pub const CRON_JOBS_SNAPSHOT_KEY_PREFIX: &str = "cron_jobs.";
pub const CRON_JOBS_SNAPSHOT_LAST_EVENT_SEQUENCE_KEY: &str =
    "_snapshot.cron_jobs.last_event_sequence";
pub const SCHEDULES_STREAM: &str = "CRON_SCHEDULES";
pub const SCHEDULE_SUBJECT_PREFIX: &str = "cron.schedules.";
pub const FIRE_SUBJECT_PREFIX: &str = "cron.fire.";
pub const SCHEDULE_SUBJECT_PATTERN: &str = "cron.schedules.>";
pub const FIRE_SUBJECT_PATTERN: &str = "cron.fire.>";

#[cfg(not(coverage))]
pub async fn get_or_create_cron_jobs_bucket(
    js: &jetstream::Context,
) -> Result<kv::Store, CronError> {
    get_or_create(
        js,
        kv::Config {
            bucket: CRON_JOBS_BUCKET.to_string(),
            history: 5,
            ..Default::default()
        },
    )
    .await
}

#[cfg(coverage)]
pub async fn get_or_create_cron_jobs_bucket(
    _js: &jetstream::Context,
) -> Result<kv::Store, CronError> {
    Err(CronError::kv_source(
        "coverage stub does not provision cron jobs buckets",
        std::io::Error::other(CRON_JOBS_BUCKET),
    ))
}

#[cfg(not(coverage))]
pub async fn get_or_create_snapshot_bucket(
    js: &jetstream::Context,
) -> Result<kv::Store, CronError> {
    get_or_create(
        js,
        kv::Config {
            bucket: SNAPSHOT_BUCKET.to_string(),
            history: 1,
            ..Default::default()
        },
    )
    .await
}

#[cfg(coverage)]
pub async fn get_or_create_snapshot_bucket(
    _js: &jetstream::Context,
) -> Result<kv::Store, CronError> {
    Err(CronError::kv_source(
        "coverage stub does not provision snapshot buckets",
        std::io::Error::other(SNAPSHOT_BUCKET),
    ))
}

#[cfg(not(coverage))]
pub async fn get_or_create(
    js: &jetstream::Context,
    config: kv::Config,
) -> Result<kv::Store, CronError> {
    let name = config.bucket.clone();
    match js.create_key_value(config).await {
        Ok(store) => Ok(store),
        Err(source) if is_create_key_value_already_exists(&source) => {
            js.get_key_value(&name).await.map_err(|source| {
                CronError::kv_source(
                    "failed to get existing key-value bucket after create reported already exists",
                    source,
                )
            })
        }
        Err(source) => Err(CronError::kv_source(
            "failed to create key-value bucket",
            source,
        )),
    }
}

#[cfg(coverage)]
pub async fn get_or_create(
    _js: &jetstream::Context,
    _config: kv::Config,
) -> Result<kv::Store, CronError> {
    Err(CronError::kv_source(
        "coverage stub does not provision key-value buckets",
        std::io::Error::other("coverage"),
    ))
}

#[cfg(not(coverage))]
pub async fn get_or_create_schedule_stream(
    js: &jetstream::Context,
) -> Result<stream::Stream, CronError> {
    let config = stream::Config {
        name: SCHEDULES_STREAM.to_string(),
        subjects: vec![
            SCHEDULE_SUBJECT_PATTERN.to_string(),
            FIRE_SUBJECT_PATTERN.to_string(),
        ],
        allow_message_ttl: true,
        allow_message_schedules: true,
        ..Default::default()
    };

    match js.create_stream(config).await {
        Ok(stream) => Ok(stream),
        Err(source) if is_create_stream_already_exists(&source) => {
            js.get_stream(SCHEDULES_STREAM).await.map_err(|source| {
                CronError::schedule_source(
                    "failed to get existing schedule stream after create reported already exists",
                    source,
                )
            })
        }
        Err(source) => Err(CronError::schedule_source(
            "failed to get or create schedule stream",
            source,
        )),
    }
}

#[cfg(coverage)]
pub async fn get_or_create_schedule_stream(
    _js: &jetstream::Context,
) -> Result<stream::Stream, CronError> {
    Err(CronError::schedule_source(
        "coverage stub does not provision schedule streams",
        std::io::Error::other(SCHEDULES_STREAM),
    ))
}

#[cfg(not(coverage))]
pub async fn get_or_create_events_stream(
    js: &jetstream::Context,
) -> Result<stream::Stream, CronError> {
    let config = stream::Config {
        name: EVENTS_STREAM.to_string(),
        subjects: vec![
            EVENTS_SUBJECT_PATTERN.to_string(),
            LEGACY_EVENTS_SUBJECT_PATTERN.to_string(),
        ],
        allow_atomic_publish: true,
        ..Default::default()
    };

    let stream = match js.create_stream(config.clone()).await {
        Ok(stream) => stream,
        Err(source) if is_create_stream_already_exists(&source) => {
            js.get_stream(EVENTS_STREAM).await.map_err(|source| {
                CronError::event_source(
                    "failed to get existing events stream after create reported already exists",
                    source,
                )
            })?
        }
        Err(source) => {
            return Err(CronError::event_source(
                "failed to get or create events stream",
                source,
            ));
        }
    };

    ensure_events_stream_config(js, stream, config).await
}

#[cfg(coverage)]
pub async fn get_or_create_events_stream(
    _js: &jetstream::Context,
) -> Result<stream::Stream, CronError> {
    Err(CronError::event_source(
        "coverage stub does not provision events streams",
        std::io::Error::other(EVENTS_STREAM),
    ))
}

#[cfg(not(coverage))]
async fn ensure_events_stream_config(
    js: &jetstream::Context,
    stream: stream::Stream,
    desired: stream::Config,
) -> Result<stream::Stream, CronError> {
    let mut current = stream.cached_info().config.clone();
    if current.allow_atomic_publish == desired.allow_atomic_publish
        && current.subjects == desired.subjects
    {
        return Ok(stream);
    }

    current.allow_atomic_publish = desired.allow_atomic_publish;
    current.subjects = desired.subjects;
    js.update_stream(current).await.map_err(|source| {
        CronError::event_source("failed to update events stream configuration", source)
    })?;
    js.get_stream(EVENTS_STREAM)
        .await
        .map_err(|source| CronError::event_source("failed to reopen updated events stream", source))
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
        let error = CreateKeyValueError::with_source(
            CreateKeyValueErrorKind::BucketCreate,
            stream_exists_error(),
        );

        assert!(is_create_key_value_already_exists(&error));
    }
}
