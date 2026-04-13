#![cfg_attr(coverage, allow(dead_code))]

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use crate::{
    JobId,
    config::{JobSpec, JobWriteState},
    domain::ResolvedJobSpec,
    error::CronError,
    events::{
        JobEvent, JobEventData, JobStreamState, ProjectionChange, RecordedJobEvent, apply,
        initial_state,
    },
    kv::{
        EVENTS_STREAM, EVENTS_SUBJECT_PATTERN, EVENTS_SUBJECT_PREFIX, JOBS_KEY_PREFIX,
        LEGACY_EVENTS_SUBJECT_PATTERN, LEGACY_EVENTS_SUBJECT_PREFIX, SCHEDULES_STREAM,
        get_or_create_schedule_stream,
    },
    store::JobSpecChange,
    traits::SchedulePublisher,
};
use async_nats::jetstream::{
    self,
    consumer::{AckPolicy, DeliverPolicy, ReplayPolicy, pull},
    kv,
};
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use trogon_eventsourcing::{Snapshot, SnapshotChange};

pub(crate) const NATS_BATCH_COMMIT: &str = "Nats-Batch-Commit";
pub(crate) const NATS_BATCH_ID: &str = "Nats-Batch-Id";
pub(crate) const NATS_BATCH_SEQUENCE: &str = "Nats-Batch-Sequence";

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

pub(crate) async fn rebuild_jobs_from_stream(
    stream: &jetstream::stream::Stream,
    first_sequence: u64,
    last_sequence: u64,
) -> Result<Vec<Snapshot<JobSpec>>, CronError> {
    let mut snapshots = BTreeMap::new();
    if last_sequence == 0 || first_sequence == 0 || first_sequence > last_sequence {
        return Ok(Vec::new());
    }

    for sequence in first_sequence..=last_sequence {
        let Some(message) =
            read_raw_event_message(stream, sequence, "failed to read job event from stream")
                .await?
        else {
            continue;
        };
        let version = message.sequence;
        let event = decode_recorded_job_event(message)?;
        let stream_id = job_id_from_event_subject(&event.stream_id)?;
        apply_event_to_snapshot_map(&mut snapshots, &stream_id, &event.data, version)?;
    }

    Ok(snapshots.into_values().collect())
}

pub(crate) async fn read_raw_event_message(
    stream: &jetstream::stream::Stream,
    sequence: u64,
    context: &'static str,
) -> Result<Option<async_nats::jetstream::message::StreamMessage>, CronError> {
    match stream.get_raw_message(sequence).await {
        Ok(message) => Ok(Some(message)),
        Err(source)
            if matches!(
                source.kind(),
                async_nats::jetstream::stream::RawMessageErrorKind::NoMessageFound
            ) =>
        {
            Ok(None)
        }
        Err(source) => Err(CronError::event_source(context, source)),
    }
}

fn decode_job_event_data(payload: &[u8]) -> Result<JobEventData, CronError> {
    JobEventData::decode(payload)
        .map_err(|source| CronError::event_source("failed to decode stored job event", source))
}

pub(crate) fn decode_recorded_job_event(
    message: async_nats::jetstream::message::StreamMessage,
) -> Result<RecordedJobEvent, CronError> {
    let recorded_at = recorded_at_from_message(&message)?;
    let stream_id = message.subject.to_string();
    let log_position = Some(message.sequence);
    let event = decode_job_event_data(&message.payload)?;

    Ok(event.record(stream_id, None, log_position, recorded_at))
}

pub(crate) fn decode_recorded_watch_message(
    message: &async_nats::jetstream::Message,
) -> Result<RecordedJobEvent, CronError> {
    let stream_message = async_nats::jetstream::message::StreamMessage::try_from(
        message.message.clone(),
    )
    .map_err(|source| {
        CronError::event_source(
            "failed to reconstruct stream message from watch delivery",
            source,
        )
    })?;

    decode_recorded_job_event(stream_message)
}

fn recorded_at_from_message(
    message: &async_nats::jetstream::message::StreamMessage,
) -> Result<DateTime<Utc>, CronError> {
    DateTime::<Utc>::from_timestamp(message.time.unix_timestamp(), message.time.nanosecond())
        .ok_or_else(|| {
            CronError::event_source(
                "failed to convert message timestamp into recorded event time",
                std::io::Error::other(message.subject.to_string()),
            )
        })
}

pub(crate) fn next_watch_start_sequence(last_sequence: u64) -> u64 {
    last_sequence.saturating_add(1).max(1)
}

pub(crate) fn event_watch_consumer_config(start_sequence: u64) -> pull::Config {
    pull::Config {
        deliver_policy: DeliverPolicy::ByStartSequence { start_sequence },
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        inactive_threshold: Duration::from_secs(30),
        ..Default::default()
    }
}

pub(crate) async fn ack_watch_message(message: &jetstream::Message) {
    if let Err(error) = message.ack().await {
        tracing::error!(error = %error, "Failed to acknowledge watched job event");
    }
}

pub(crate) async fn apply_projection_change(
    kv: &kv::Store,
    change: &ProjectionChange,
) -> Result<(), CronError> {
    match change {
        ProjectionChange::Upsert(job) => {
            let key = format!("{JOBS_KEY_PREFIX}{}", job.id);
            let value = serde_json::to_vec(job)?;
            kv.put(key, value.into()).await.map_err(|source| {
                CronError::kv_source("failed to store projected job state", source)
            })?;
        }
        ProjectionChange::Delete(id) => {
            kv.delete(format!("{JOBS_KEY_PREFIX}{id}"))
                .await
                .map_err(|source| {
                    CronError::kv_source("failed to delete projected job state", source)
                })?;
        }
    }

    Ok(())
}

pub(crate) fn change_from_projection_change(change: ProjectionChange) -> JobSpecChange {
    match change {
        ProjectionChange::Upsert(job) => JobSpecChange::Put(job),
        ProjectionChange::Delete(id) => JobSpecChange::Delete(id),
    }
}

pub(crate) fn apply_event_to_snapshot_map(
    snapshots: &mut BTreeMap<String, Snapshot<JobSpec>>,
    stream_id: &JobId,
    event: &JobEvent,
    version: u64,
) -> Result<SnapshotChange<JobSpec>, CronError> {
    ensure_event_matches_stream(stream_id, event)?;
    let current_state = match snapshots.get(stream_id.as_str()).cloned() {
        Some(snapshot) => JobStreamState::try_from(snapshot).map_err(|source| {
            CronError::event_source(
                "failed to decode job snapshot into stream state during projection",
                source,
            )
        })?,
        None => initial_state(),
    };
    let next_state = apply(current_state, event.clone()).map_err(|source| {
        CronError::event_source("failed to apply job event to stream snapshot state", source)
    })?;

    match next_state {
        JobStreamState::Present(spec) => {
            let snapshot = Snapshot::new(version, spec);
            snapshots.insert(stream_id.to_string(), snapshot.clone());
            Ok(SnapshotChange::upsert(stream_id.to_string(), snapshot))
        }
        JobStreamState::Initial => {
            snapshots.remove(stream_id.as_str());
            Ok(SnapshotChange::delete(stream_id.to_string()))
        }
    }
}

pub(crate) fn job_id_from_event_subject(subject: &str) -> Result<JobId, CronError> {
    let raw_id = subject
        .strip_prefix(EVENTS_SUBJECT_PREFIX)
        .or_else(|| subject.strip_prefix(LEGACY_EVENTS_SUBJECT_PREFIX))
        .ok_or_else(|| {
            CronError::event_source(
                "failed to derive job stream id from event subject",
                std::io::Error::other(subject.to_string()),
            )
        })?;

    JobId::parse(raw_id).map_err(|source| {
        CronError::event_source("failed to parse job stream id from event subject", source)
    })
}

pub(crate) fn ensure_event_matches_stream(
    expected_stream_id: &JobId,
    event: &JobEvent,
) -> Result<(), CronError> {
    if event.job_id() == expected_stream_id.as_str() {
        Ok(())
    } else {
        Err(CronError::event_source(
            "event payload stream id does not match the expected stream",
            std::io::Error::other(format!(
                "expected '{}' but event carried '{}'",
                expected_stream_id,
                event.job_id()
            )),
        ))
    }
}

pub(crate) fn job_stream_state_from_snapshot(
    expected_stream_id: &JobId,
    snapshot: Snapshot<JobSpec>,
    context: &'static str,
) -> Result<JobStreamState, CronError> {
    if snapshot.payload.id != expected_stream_id.as_str() {
        return Err(CronError::event_source(
            context,
            std::io::Error::other(format!(
                "expected '{}' but snapshot carried '{}'",
                expected_stream_id, snapshot.payload.id
            )),
        ));
    }

    JobStreamState::try_from(snapshot).map_err(|source| CronError::event_source(context, source))
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
    use crate::config::{DeliverySpec, JobEnabledState, JobSpec, JobWriteCondition, ScheduleSpec};
    use crate::events::{JobStreamState, apply, projection_change};

    fn test_job(id: &str) -> JobSpec {
        JobSpec {
            id: id.to_string(),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::Every { every_sec: 30 },
            delivery: DeliverySpec::NatsEvent {
                route: "agent.run".to_string(),
                headers: Default::default(),
                ttl_sec: None,
                source: None,
            },
            payload: serde_json::json!({"kind": "heartbeat"}),
            metadata: Default::default(),
        }
    }

    #[test]
    fn parse_server_version_supports_dev_suffixes() {
        assert_eq!(parse_server_version("2.14.0-dev"), Some((2, 14)));
        assert_eq!(parse_server_version("2.12.6"), Some((2, 12)));
        assert_eq!(parse_server_version("garbage"), None);
    }

    #[test]
    fn live_projection_applies_state_change() {
        let current = JobStreamState::Present(test_job("alpha"));
        let next = apply(
            current.clone(),
            JobEvent::job_state_changed("alpha", JobEnabledState::Disabled),
        )
        .unwrap();
        let change = projection_change(&current, &next).unwrap();

        match change {
            ProjectionChange::Upsert(job) => assert_eq!(job.state, JobEnabledState::Disabled),
            ProjectionChange::Delete(_) => panic!("expected upsert change"),
        }
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
    fn deleted_streams_can_be_recreated_without_changing_subject() {
        let state =
            resolve_event_subject_state("alpha", Some(JobWriteState::new(Some(12), false)), None)
                .unwrap();

        assert_eq!(state.prefix, EventSubjectPrefix::Canonical);
        assert_eq!(state.write_state.current_version(), Some(12));
        assert!(!state.write_state.exists());
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

    #[test]
    fn watch_start_sequence_moves_past_bootstrap_tail() {
        assert_eq!(next_watch_start_sequence(0), 1);
        assert_eq!(next_watch_start_sequence(41), 42);
    }

    #[test]
    fn watch_consumer_replays_only_after_bootstrap_boundary() {
        let config = event_watch_consumer_config(42);

        assert_eq!(
            config.deliver_policy,
            DeliverPolicy::ByStartSequence { start_sequence: 42 }
        );
        assert_eq!(config.ack_policy, AckPolicy::Explicit);
        assert_eq!(config.replay_policy, ReplayPolicy::Instant);
    }

    #[test]
    fn snapshot_projection_replays_delete_and_recreate() {
        let mut snapshots = BTreeMap::new();
        let stream_id = JobId::parse("alpha").unwrap();

        apply_event_to_snapshot_map(
            &mut snapshots,
            &stream_id,
            &JobEvent::job_registered(test_job("alpha")),
            1,
        )
        .unwrap();
        apply_event_to_snapshot_map(
            &mut snapshots,
            &stream_id,
            &JobEvent::job_state_changed("alpha", JobEnabledState::Disabled),
            2,
        )
        .unwrap();
        apply_event_to_snapshot_map(
            &mut snapshots,
            &stream_id,
            &JobEvent::job_removed("alpha"),
            3,
        )
        .unwrap();
        apply_event_to_snapshot_map(
            &mut snapshots,
            &stream_id,
            &JobEvent::job_registered(test_job("alpha")),
            4,
        )
        .unwrap();

        let snapshot = snapshots.get("alpha").unwrap();
        assert_eq!(snapshot.version, 4);
        assert_eq!(snapshot.payload, test_job("alpha"));
    }

    #[test]
    fn snapshot_projection_rejects_invalid_transition_sequence() {
        let stream_id = JobId::parse("alpha").unwrap();
        let error = apply_event_to_snapshot_map(
            &mut BTreeMap::new(),
            &stream_id,
            &JobEvent::job_state_changed("alpha", JobEnabledState::Disabled),
            1,
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("failed to apply job event to stream snapshot state")
        );
    }
}
