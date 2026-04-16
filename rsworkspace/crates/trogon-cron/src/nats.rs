#![cfg_attr(coverage, allow(dead_code))]

use std::collections::HashSet;

use crate::{
    config::JobWriteState,
    domain::ResolvedJobSpec,
    error::CronError,
    kv::{
        EVENTS_STREAM, EVENTS_SUBJECT_PATTERN, EVENTS_SUBJECT_PREFIX,
        LEGACY_EVENTS_SUBJECT_PATTERN, LEGACY_EVENTS_SUBJECT_PREFIX, SCHEDULES_STREAM,
        get_or_create_schedule_stream,
    },
    traits::SchedulePublisher,
};
use async_nats::jetstream::{self};
use futures::TryStreamExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventSubjectPrefix {
    Canonical,
    Legacy,
}

impl EventSubjectPrefix {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Canonical => EVENTS_SUBJECT_PREFIX,
            Self::Legacy => LEGACY_EVENTS_SUBJECT_PREFIX,
        }
    }

    pub(crate) fn subject(self, job_id: &str) -> String {
        format!("{}{}", self.as_str(), job_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StreamSubjectState {
    pub(crate) prefix: EventSubjectPrefix,
    pub(crate) write_state: JobWriteState,
}

pub(crate) fn resolve_event_subject_state(
    job_id: &str,
    canonical_state: Option<JobWriteState>,
    legacy_state: Option<JobWriteState>,
) -> Result<StreamSubjectState, CronError> {
    match (canonical_state, legacy_state) {
        (Some(_), Some(_)) => Err(CronError::event_source(
            "job stream has conflicting event subjects",
            std::io::Error::other(format!("job '{job_id}'")),
        )),
        (Some(write_state), None) => Ok(StreamSubjectState {
            prefix: EventSubjectPrefix::Canonical,
            write_state,
        }),
        (None, Some(write_state)) => Ok(StreamSubjectState {
            prefix: EventSubjectPrefix::Legacy,
            write_state,
        }),
        (None, None) => Ok(StreamSubjectState {
            prefix: EventSubjectPrefix::Canonical,
            write_state: JobWriteState::new(None, false),
        }),
    }
}

#[derive(Clone)]
pub struct NatsSchedulePublisher {
    js: jetstream::Context,
}

impl NatsSchedulePublisher {
    pub async fn new(nats: async_nats::Client) -> Result<Self, CronError> {
        validate_server_version(&nats)?;
        let js = jetstream::new(nats);
        validate_schedule_stream(&get_or_create_schedule_stream(&js).await?)?;
        Ok(Self { js })
    }

    async fn stream(&self) -> Result<jetstream::stream::Stream, CronError> {
        self.js
            .get_stream(SCHEDULES_STREAM)
            .await
            .map_err(|source| CronError::schedule_source("failed to open schedule stream", source))
    }

    async fn ensure_source_subject(&self, source_subject: Option<&str>) -> Result<(), CronError> {
        let Some(source_subject) = source_subject else {
            return Ok(());
        };

        let stream = self.stream().await?;
        let mut config = stream
            .get_info()
            .await
            .map_err(|source| {
                CronError::schedule_source(
                    "failed to query schedule stream info for sampling source",
                    source,
                )
            })?
            .config;
        if config
            .subjects
            .iter()
            .any(|subject| subject == source_subject)
        {
            return Ok(());
        }

        if let Ok(source_stream) = self.js.stream_by_subject(source_subject).await
            && source_stream != SCHEDULES_STREAM
        {
            return Err(CronError::schedule_source(
                "sampling source subject is already claimed by another stream and cannot be used with the scheduler-owned stream topology",
                std::io::Error::other(format!(
                    "subject '{source_subject}' belongs to stream '{source_stream}'"
                )),
            ));
        }

        config.subjects.push(source_subject.to_string());
        self.js.update_stream(config).await.map_err(|source| {
            CronError::schedule_source(
                "failed to update schedule stream for sampling source",
                source,
            )
        })?;

        Ok(())
    }

    async fn active_schedule_ids(&self) -> Result<HashSet<String>, CronError> {
        let stream = self.stream().await?;
        let mut info = stream
            .info_with_subjects(crate::kv::SCHEDULE_SUBJECT_PATTERN)
            .await
            .map_err(|source| {
                CronError::schedule_source("failed to query active schedule subjects", source)
            })?;
        let mut ids = HashSet::new();

        while let Some((subject, _count)) = info.try_next().await.map_err(|source| {
            CronError::schedule_source("failed to iterate active schedule subjects", source)
        })? {
            if let Some(job_id) = subject.strip_prefix(crate::kv::SCHEDULE_SUBJECT_PREFIX)
                && !job_id.is_empty()
            {
                ids.insert(job_id.to_string());
            }
        }

        Ok(ids)
    }
}

impl SchedulePublisher for NatsSchedulePublisher {
    type Error = CronError;

    async fn active_schedule_ids(&self) -> Result<HashSet<String>, Self::Error> {
        self.active_schedule_ids().await
    }

    async fn upsert_schedule(&self, job: &ResolvedJobSpec) -> Result<(), Self::Error> {
        self.ensure_source_subject(job.source_subject()).await?;

        let ack = self
            .js
            .publish_with_headers(
                job.schedule_subject().to_string(),
                job.schedule_headers(),
                job.schedule_body(),
            )
            .await
            .map_err(|source| {
                CronError::schedule_source("failed to publish native schedule", source)
            })?;

        ack.await.map_err(|source| {
            CronError::schedule_source("failed to acknowledge native schedule", source)
        })?;

        Ok(())
    }

    async fn remove_schedule(&self, job_id: &str) -> Result<(), Self::Error> {
        let job_id = trogon_nats::NatsToken::new(job_id).map_err(|source| {
            CronError::invalid_job_spec(crate::error::JobSpecError::InvalidId {
                id: job_id.to_string(),
                source,
            })
        })?;
        self.stream()
            .await?
            .purge()
            .filter(format!("cron.schedules.{}", job_id.as_str()))
            .await
            .map_err(|source| {
                CronError::schedule_source("failed to purge native schedule", source)
            })?;
        Ok(())
    }
}

fn validate_server_version(nats: &async_nats::Client) -> Result<(), CronError> {
    let version = nats.server_info().version;
    let supported =
        parse_server_version(&version).is_some_and(|(major, minor)| (major, minor) >= (2, 14));
    if supported {
        tracing::info!(server_version = %version, "Using NATS scheduler feature line");
        Ok(())
    } else {
        Err(CronError::schedule_source(
            "connected NATS server does not satisfy the required 2.14 feature line",
            std::io::Error::other(format!("reported version: {version}")),
        ))
    }
}

fn parse_server_version(version: &str) -> Option<(u64, u64)> {
    let core = version.split('-').next().unwrap_or(version);
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

pub(crate) fn validate_events_stream(stream: &jetstream::stream::Stream) -> Result<(), CronError> {
    let config = &stream.cached_info().config;
    if !config.allow_atomic_publish {
        return Err(CronError::event_source(
            "events stream is missing allow_atomic",
            std::io::Error::other(EVENTS_STREAM),
        ));
    }
    if !config
        .subjects
        .iter()
        .any(|subject| subject == EVENTS_SUBJECT_PATTERN)
    {
        return Err(CronError::event_source(
            "events stream is missing canonical job event subject coverage",
            std::io::Error::other(EVENTS_STREAM),
        ));
    }
    if !config
        .subjects
        .iter()
        .any(|subject| subject == LEGACY_EVENTS_SUBJECT_PATTERN)
    {
        return Err(CronError::event_source(
            "events stream is missing legacy job event subject coverage",
            std::io::Error::other(EVENTS_STREAM),
        ));
    }
    Ok(())
}

fn validate_schedule_stream(stream: &jetstream::stream::Stream) -> Result<(), CronError> {
    let config = &stream.cached_info().config;
    if !config.allow_message_schedules {
        return Err(CronError::schedule_source(
            "schedule stream is missing allow_msg_schedules",
            std::io::Error::other(SCHEDULES_STREAM),
        ));
    }
    if !config.allow_message_ttl {
        return Err(CronError::schedule_source(
            "schedule stream is missing allow_msg_ttl",
            std::io::Error::other(SCHEDULES_STREAM),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::JobWriteCondition;

    #[test]
    fn parse_server_version_supports_dev_suffixes() {
        assert_eq!(parse_server_version("2.14.0-dev"), Some((2, 14)));
        assert_eq!(parse_server_version("2.12.6"), Some((2, 12)));
        assert_eq!(parse_server_version("garbage"), None);
    }

    #[test]
    fn write_condition_rejects_unexpected_version() {
        let error = JobWriteCondition::MustBeAtVersion(3)
            .ensure("alpha", JobWriteState::new(Some(4), true))
            .unwrap_err();

        assert!(matches!(
            error,
            CronError::OptimisticConcurrencyConflict {
                current_version: Some(4),
                ..
            }
        ));
    }

    #[test]
    fn new_streams_use_canonical_event_subject() {
        let state = resolve_event_subject_state("alpha", None, None).unwrap();

        assert_eq!(state.prefix, EventSubjectPrefix::Canonical);
        assert_eq!(state.write_state.current_version(), None);
        assert!(!state.write_state.exists());
    }

    #[test]
    fn legacy_streams_keep_legacy_event_subject() {
        let state =
            resolve_event_subject_state("alpha", None, Some(JobWriteState::new(Some(12), true)))
                .unwrap();

        assert_eq!(state.prefix, EventSubjectPrefix::Legacy);
        assert_eq!(state.write_state.current_version(), Some(12));
        assert!(state.write_state.exists());
    }

    #[test]
    fn deleted_streams_keep_their_subject_and_still_count_as_existing() {
        let state =
            resolve_event_subject_state("alpha", Some(JobWriteState::new(Some(12), true)), None)
                .unwrap();

        assert_eq!(state.prefix, EventSubjectPrefix::Canonical);
        assert_eq!(state.write_state.current_version(), Some(12));
        assert!(state.write_state.exists());
    }

    #[test]
    fn split_stream_subjects_are_rejected() {
        let error = resolve_event_subject_state(
            "alpha",
            Some(JobWriteState::new(Some(7), true)),
            Some(JobWriteState::new(Some(9), true)),
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("job stream has conflicting event subjects")
        );
    }
}
