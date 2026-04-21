use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_nats::jetstream::{
    self,
    consumer::{AckPolicy, DeliverPolicy, ReplayPolicy, pull},
    kv,
};
use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt, future};
use trogon_eventsourcing::nats::{
    jetstream::AppendProjector,
    kv::{
        load_snapshot, load_snapshot_map, maybe_advance_checkpoint, persist_snapshot_change,
        read_checkpoint, write_checkpoint,
    },
};
use trogon_eventsourcing::snapshot::{Snapshot, SnapshotChange};
use trogon_eventsourcing::{EventData, StreamEvent};
use trogon_nats::SubjectTokenViolation;
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream};

use crate::{
    CronJob, JobId,
    error::CronError,
    events::{
        JobAdded, JobEvent, JobEventCodec, JobEventData, JobEventState, JobPaused, JobRemoved,
        JobResumed, RecordedJobEvent,
    },
    kv::{EVENTS_SUBJECT_PREFIX, LEGACY_EVENTS_SUBJECT_PREFIX},
    store::{
        open_cron_jobs_bucket, open_events_stream, open_snapshot_bucket, snapshot_store_config,
    },
};

pub type CronJobWatchStream = Pin<Box<dyn Stream<Item = CronJobChange> + Send + 'static>>;
pub type LoadAndWatchCronJobsResult = Result<(Vec<CronJob>, CronJobWatchStream), CronError>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CronJobSnapshotProjector;

impl AppendProjector<JobId> for CronJobSnapshotProjector {
    type Error = CronError;

    async fn project_appended(
        &self,
        snapshot_bucket: &kv::Store,
        stream_id: &JobId,
        events: &[EventData],
        next_expected_version: u64,
    ) -> Result<(), Self::Error> {
        project_appended_events(
            snapshot_bucket,
            stream_id.as_str(),
            events,
            next_expected_version,
        )
        .await
    }
}

#[derive(Debug, Clone)]
pub enum CronJobChange {
    Put(CronJob),
    Delete(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProjectionChange {
    Upsert(CronJob),
    Delete(String),
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum JobStreamState {
    Initial,
    Present(CronJob),
    Deleted(String),
}

#[derive(Debug)]
pub enum JobTransitionError {
    InvalidEventId {
        id: String,
        source: SubjectTokenViolation,
    },
    CannotAddExistingJob {
        id: String,
    },
    CannotAddDeletedJob {
        id: String,
    },
    MissingJobForStateChange {
        id: String,
    },
    DeletedJobForStateChange {
        id: String,
    },
    DeletedJobForRemoval {
        id: String,
    },
}

pub const fn initial_state() -> JobStreamState {
    JobStreamState::Initial
}

pub fn apply(state: JobStreamState, event: JobEvent) -> Result<JobStreamState, JobTransitionError> {
    match (state, event) {
        (JobStreamState::Initial, JobEvent::JobAdded(JobAdded { id, job })) => {
            validate_event_job_id(&id).map_err(|source| JobTransitionError::InvalidEventId {
                id: id.clone(),
                source,
            })?;
            Ok(JobStreamState::Present(CronJob::from((id, job))))
        }
        (JobStreamState::Initial, event @ JobEvent::JobPaused(JobPaused { .. })) => {
            Err(JobTransitionError::MissingJobForStateChange {
                id: parse_event_job_id(&event)?,
            })
        }
        (JobStreamState::Initial, event @ JobEvent::JobResumed(JobResumed { .. })) => {
            Err(JobTransitionError::MissingJobForStateChange {
                id: parse_event_job_id(&event)?,
            })
        }
        (JobStreamState::Initial, event @ JobEvent::JobRemoved(JobRemoved { .. })) => {
            Ok(JobStreamState::Deleted(parse_event_job_id(&event)?))
        }
        (JobStreamState::Present(job), JobEvent::JobAdded(JobAdded { .. })) => {
            Err(JobTransitionError::CannotAddExistingJob { id: job.id })
        }
        (JobStreamState::Present(mut job), JobEvent::JobPaused(JobPaused { .. })) => {
            job.state = JobEventState::Disabled;
            Ok(JobStreamState::Present(job))
        }
        (JobStreamState::Present(mut job), JobEvent::JobResumed(JobResumed { .. })) => {
            job.state = JobEventState::Enabled;
            Ok(JobStreamState::Present(job))
        }
        (JobStreamState::Present(job), JobEvent::JobRemoved(JobRemoved { .. })) => {
            Ok(JobStreamState::Deleted(job.id))
        }
        (JobStreamState::Deleted(id), JobEvent::JobAdded(JobAdded { .. })) => {
            Err(JobTransitionError::CannotAddDeletedJob { id })
        }
        (JobStreamState::Deleted(id), JobEvent::JobPaused(JobPaused { .. })) => {
            Err(JobTransitionError::DeletedJobForStateChange { id })
        }
        (JobStreamState::Deleted(id), JobEvent::JobResumed(JobResumed { .. })) => {
            Err(JobTransitionError::DeletedJobForStateChange { id })
        }
        (JobStreamState::Deleted(id), JobEvent::JobRemoved(JobRemoved { .. })) => {
            Err(JobTransitionError::DeletedJobForRemoval { id })
        }
    }
}

pub fn projection_change(
    before: &JobStreamState,
    after: &JobStreamState,
) -> Option<ProjectionChange> {
    match (before, after) {
        (JobStreamState::Initial, JobStreamState::Initial) => None,
        (_, JobStreamState::Present(spec)) => Some(ProjectionChange::Upsert(spec.clone())),
        (JobStreamState::Present(spec), JobStreamState::Initial | JobStreamState::Deleted(_)) => {
            Some(ProjectionChange::Delete(spec.id.to_string()))
        }
        (JobStreamState::Initial, JobStreamState::Deleted(_))
        | (JobStreamState::Deleted(_), JobStreamState::Initial)
        | (JobStreamState::Deleted(_), JobStreamState::Deleted(_)) => None,
    }
}

impl JobStreamState {
    pub fn into_job(self) -> Option<CronJob> {
        match self {
            Self::Initial => None,
            Self::Deleted(_) => None,
            Self::Present(job) => Some(job),
        }
    }

    pub fn into_snapshot(self, version: u64) -> Option<Snapshot<CronJob>> {
        self.into_job().map(|job| Snapshot::new(version, job))
    }
}

impl From<CronJob> for JobStreamState {
    fn from(job: CronJob) -> Self {
        Self::Present(job)
    }
}

impl From<Snapshot<CronJob>> for JobStreamState {
    fn from(value: Snapshot<CronJob>) -> Self {
        Self::Present(value.payload)
    }
}

impl std::fmt::Display for JobTransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEventId { id, .. } => write!(f, "job event id '{id}' is invalid"),
            Self::CannotAddExistingJob { id } => write!(f, "job '{id}' already exists"),
            Self::CannotAddDeletedJob { id } => {
                write!(f, "job '{id}' was deleted and cannot be added again")
            }
            Self::MissingJobForStateChange { id } => {
                write!(f, "missing job for state change '{id}'")
            }
            Self::DeletedJobForStateChange { id } => {
                write!(f, "deleted job '{id}' cannot change state")
            }
            Self::DeletedJobForRemoval { id } => {
                write!(f, "job '{id}' was already deleted")
            }
        }
    }
}

impl std::error::Error for JobTransitionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidEventId { .. } => None,
            _ => None,
        }
    }
}

pub async fn load_and_watch_cron_jobs<J>(js: &J) -> LoadAndWatchCronJobsResult
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    let stream: jetstream::stream::Stream = open_events_stream(js).await?;
    let info = stream
        .get_info()
        .await
        .map_err(|source| CronError::event_source("failed to query events stream info", source))?;
    let last_sequence = info.state.last_sequence;
    let initial_jobs =
        rebuild_jobs_from_stream(&stream, info.state.first_sequence, last_sequence).await?;
    rewrite_cron_jobs_projection(js, &initial_jobs).await?;
    let consumer = stream
        .create_consumer(event_watch_consumer_config(next_watch_start_sequence(
            last_sequence,
        )))
        .await
        .map_err(|source| {
            CronError::event_source("failed to create cron job event watch consumer", source)
        })?;
    let subscriber = consumer.messages().await.map_err(|source| {
        CronError::event_source("failed to open cron job event watch stream", source)
    })?;

    let kv: kv::Store = open_cron_jobs_bucket(js).await?;
    let state = initial_jobs
        .iter()
        .cloned()
        .map(|job| {
            (
                job.payload.id.to_string(),
                JobStreamState::Present(job.payload),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let state = Arc::new(Mutex::new(state));
    let watcher: CronJobWatchStream = Box::pin(subscriber.then(move |result| {
        let state = Arc::clone(&state);
        let kv = kv.clone();
        async move {
            let message = match result {
                Ok(message) => message,
                Err(error) => {
                    tracing::error!(error = %error, "Failed to read cron job event from watch consumer");
                    return None;
                }
            };

            let event = match decode_recorded_watch_message(&message) {
                Ok(event) => event,
                Err(error) => {
                    tracing::error!(error = %error, "Failed to decode cron job event from watcher");
                    ack_watch_message(&message).await;
                    return None;
                }
            };

            let stream_id = match job_id_from_event_subject(&event.recorded_stream_id) {
                Ok(stream_id) => stream_id,
                Err(error) => {
                    tracing::error!(error = %error, "Failed to derive watched cron job stream id from subject");
                    ack_watch_message(&message).await;
                    return None;
                }
            };
            let data = match event.decode_data() {
                Ok(data) => data,
                Err(error) => {
                    tracing::error!(error = %error, "Failed to decode watched cron job event payload");
                    ack_watch_message(&message).await;
                    return None;
                }
            };
            if let Err(error) = ensure_event_matches_stream(&stream_id, &data) {
                tracing::error!(error = %error, "Watched cron job event payload does not match stream subject");
                ack_watch_message(&message).await;
                return None;
            }
            let projection_change = {
                let Some(mut state) = (match state.lock() {
                    Ok(state) => Some(state),
                    Err(source) => {
                        tracing::error!(error = %source, "Cron jobs projection state mutex poisoned");
                        None
                    }
                }) else {
                    ack_watch_message(&message).await;
                    return None;
                };
                (|| -> Result<Option<ProjectionChange>, CronError> {
                    let current = state
                        .get(stream_id.as_str())
                        .cloned()
                        .unwrap_or_else(initial_state);
                    let next = apply(current.clone(), data.clone()).map_err(|error| {
                        CronError::event_source(
                            "failed to apply watched job event to stream state",
                            error,
                        )
                    })?;
                    let change = projection_change(&current, &next);
                    match &next {
                        JobStreamState::Present(_) | JobStreamState::Deleted(_) => {
                            state.insert(stream_id.to_string(), next);
                        }
                        JobStreamState::Initial => {
                            state.remove(stream_id.as_str());
                        }
                    }

                    Ok(change)
                })()
            };
            let projection_change = match projection_change {
                Ok(change) => change,
                Err(error) => {
                    tracing::error!(error = %error, "Failed to apply job event to current state");
                    ack_watch_message(&message).await;
                    return None;
                }
            };
            let Some(projection_change) = projection_change else {
                ack_watch_message(&message).await;
                return None;
            };

            if let Err(error) = apply_projection_change(&kv, &projection_change).await {
                tracing::error!(error = %error, "Failed to update projected cron jobs state from event");
                ack_watch_message(&message).await;
                return None;
            }

            ack_watch_message(&message).await;
            Some(change_from_projection_change(projection_change))
        }
    }).filter_map(future::ready));

    Ok((
        initial_jobs.into_iter().map(|job| job.payload).collect(),
        watcher,
    ))
}

pub(crate) async fn catch_up_snapshots<J>(js: &J) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    let stream: jetstream::stream::Stream = open_events_stream(js).await?;
    let info = stream.get_info().await.map_err(|source| {
        CronError::event_source(
            "failed to query events stream info for snapshot catch-up",
            source,
        )
    })?;
    if info.state.messages == 0 {
        return Ok(());
    }

    let bucket = open_snapshot_bucket(js).await?;
    let checkpoint = read_checkpoint(&bucket, &snapshot_store_config())
        .await
        .map_err(CronError::from)?;
    if checkpoint >= info.state.last_sequence {
        return Ok(());
    }

    let mut snapshots = load_snapshot_map(&bucket, &snapshot_store_config())
        .await
        .map_err(CronError::from)?;
    let mut states = snapshot_state_map(&snapshots);
    let start = checkpoint.max(info.state.first_sequence.saturating_sub(1)) + 1;

    for sequence in start..=info.state.last_sequence {
        let Some(message) = read_raw_event_message(
            &stream,
            sequence,
            "failed to read job event during snapshot catch-up",
        )
        .await?
        else {
            continue;
        };
        let event = decode_recorded_job_event(message)?;
        let stream_id = job_id_from_event_subject(&event.recorded_stream_id)?;
        let data = event.decode_data().map_err(|source| {
            CronError::event_source(
                "failed to decode job event during snapshot catch-up",
                source,
            )
        })?;
        let change =
            apply_event_to_snapshot_map(&mut states, &mut snapshots, &stream_id, &data, sequence)?;
        persist_snapshot_change(&bucket, &snapshot_store_config(), change)
            .await
            .map_err(CronError::from)?;
        write_checkpoint(&bucket, &snapshot_store_config(), sequence)
            .await
            .map_err(CronError::from)?;
    }

    Ok(())
}

pub(crate) async fn project_appended_events(
    bucket: &kv::Store,
    job_id: &str,
    events: &[JobEventData],
    final_version: u64,
) -> Result<(), CronError> {
    if events.is_empty() {
        return Ok(());
    }
    validate_event_job_id(job_id).map_err(|source| {
        CronError::invalid_job_spec(crate::JobSpecError::InvalidId {
            id: job_id.to_string(),
            source,
        })
    })?;

    let mut snapshots = BTreeMap::new();
    let mut states = BTreeMap::new();
    if let Some(snapshot) = load_snapshot(bucket, &snapshot_store_config(), job_id)
        .await
        .map_err(CronError::from)?
    {
        states.insert(job_id.to_string(), JobStreamState::from(snapshot.clone()));
        snapshots.insert(job_id.to_string(), snapshot);
    }

    let start_version = final_version
        .checked_sub(events.len() as u64 - 1)
        .ok_or_else(|| {
            CronError::event_source(
                "stream snapshot projection requires a valid batch version range",
                std::io::Error::other(format!("job '{job_id}'")),
            )
        })?;

    for (index, event) in events.iter().enumerate() {
        let decoded = event.decode_data().map_err(|source| {
            CronError::event_source("failed to decode job event for snapshot projection", source)
        })?;
        let change = apply_event_to_snapshot_map(
            &mut states,
            &mut snapshots,
            job_id,
            &decoded,
            start_version + index as u64,
        )?;
        persist_snapshot_change(bucket, &snapshot_store_config(), change)
            .await
            .map_err(CronError::from)?;
    }
    maybe_advance_checkpoint(bucket, &snapshot_store_config(), final_version)
        .await
        .map_err(CronError::from)
}

fn parse_event_job_id(event: &JobEvent) -> Result<String, JobTransitionError> {
    let id = event.stream_id().to_string();
    validate_event_job_id(&id)
        .map(|()| id.clone())
        .map_err(|source| JobTransitionError::InvalidEventId { id, source })
}

async fn rewrite_cron_jobs_projection<J>(
    js: &J,
    jobs: &[Snapshot<CronJob>],
) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    let kv: kv::Store = open_cron_jobs_bucket(js).await?;
    let mut keys = kv
        .keys()
        .await
        .map_err(|source| CronError::kv_source("failed to list projection keys", source))?;

    while let Some(result) = keys.next().await {
        let key = result
            .map_err(|source| CronError::kv_source("failed to read projection key", source))?;
        let _ = kv.purge(key).await;
    }

    for job in jobs {
        let value = serde_json::to_vec(&job.payload)?;
        kv.put(job.payload.id.to_string(), value.into())
            .await
            .map_err(|source| {
                CronError::kv_source("failed to write projected job state", source)
            })?;
    }

    Ok(())
}

async fn rebuild_jobs_from_stream(
    stream: &jetstream::stream::Stream,
    first_sequence: u64,
    last_sequence: u64,
) -> Result<Vec<Snapshot<CronJob>>, CronError> {
    let mut snapshots = BTreeMap::new();
    let mut states = BTreeMap::new();
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
        let stream_id = job_id_from_event_subject(&event.recorded_stream_id)?;
        let data = event.decode_data_with(&JobEventCodec).map_err(|source| {
            CronError::event_source("failed to decode recorded job event payload", source)
        })?;
        apply_event_to_snapshot_map(&mut states, &mut snapshots, &stream_id, &data, version)?;
    }

    Ok(snapshots.into_values().collect())
}

async fn read_raw_event_message(
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

fn decode_recorded_job_event(
    message: async_nats::jetstream::message::StreamMessage,
) -> Result<RecordedJobEvent, CronError> {
    let recorded_at = recorded_at_from_message(&message)?;
    let stream_id = message.subject.to_string();
    let log_position = Some(message.sequence);
    let event = decode_job_event_data(&message.payload)?;

    Ok(event.record(stream_id, None, log_position, recorded_at))
}

fn decode_recorded_watch_message(
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

fn next_watch_start_sequence(last_sequence: u64) -> u64 {
    last_sequence.saturating_add(1).max(1)
}

fn event_watch_consumer_config(start_sequence: u64) -> pull::Config {
    pull::Config {
        deliver_policy: DeliverPolicy::ByStartSequence { start_sequence },
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        inactive_threshold: Duration::from_secs(30),
        ..Default::default()
    }
}

async fn ack_watch_message(message: &jetstream::Message) {
    if let Err(error) = message.ack().await {
        tracing::error!(error = %error, "Failed to acknowledge watched job event");
    }
}

async fn apply_projection_change(
    kv: &kv::Store,
    change: &ProjectionChange,
) -> Result<(), CronError> {
    match change {
        ProjectionChange::Upsert(job) => {
            let value = serde_json::to_vec(job)?;
            kv.put(job.id.to_string(), value.into())
                .await
                .map_err(|source| {
                    CronError::kv_source("failed to store projected job state", source)
                })?;
        }
        ProjectionChange::Delete(id) => {
            kv.delete(id.clone()).await.map_err(|source| {
                CronError::kv_source("failed to delete projected job state", source)
            })?;
        }
    }

    Ok(())
}

fn change_from_projection_change(change: ProjectionChange) -> CronJobChange {
    match change {
        ProjectionChange::Upsert(job) => CronJobChange::Put(job),
        ProjectionChange::Delete(id) => CronJobChange::Delete(id),
    }
}

fn apply_event_to_snapshot_map(
    states: &mut BTreeMap<String, JobStreamState>,
    snapshots: &mut BTreeMap<String, Snapshot<CronJob>>,
    stream_id: &str,
    event: &JobEvent,
    version: u64,
) -> Result<SnapshotChange<CronJob>, CronError> {
    ensure_event_matches_stream(stream_id, event)?;
    let current_state = states.get(stream_id).cloned().unwrap_or_else(initial_state);
    let next_state = apply(current_state, event.clone()).map_err(|source| {
        CronError::event_source("failed to apply job event to stream snapshot state", source)
    })?;

    states.insert(stream_id.to_string(), next_state.clone());

    match next_state {
        JobStreamState::Present(job) => {
            let snapshot = Snapshot::new(version, job);
            snapshots.insert(stream_id.to_string(), snapshot.clone());
            Ok(SnapshotChange::upsert(stream_id.to_string(), snapshot))
        }
        JobStreamState::Initial | JobStreamState::Deleted(_) => {
            snapshots.remove(stream_id);
            Ok(SnapshotChange::delete(stream_id.to_string()))
        }
    }
}

fn snapshot_state_map(
    snapshots: &BTreeMap<String, Snapshot<CronJob>>,
) -> BTreeMap<String, JobStreamState> {
    snapshots
        .iter()
        .map(|(stream_id, snapshot)| (stream_id.clone(), JobStreamState::from(snapshot.clone())))
        .collect()
}

fn job_id_from_event_subject(subject: &str) -> Result<String, CronError> {
    let raw_id = subject
        .strip_prefix(EVENTS_SUBJECT_PREFIX)
        .or_else(|| subject.strip_prefix(LEGACY_EVENTS_SUBJECT_PREFIX))
        .ok_or_else(|| {
            CronError::event_source(
                "failed to derive job stream id from event subject",
                std::io::Error::other(subject.to_string()),
            )
        })?;

    validate_event_job_id(raw_id)
        .map(|()| raw_id.to_string())
        .map_err(|source| {
            CronError::invalid_job_spec(crate::JobSpecError::InvalidId {
                id: raw_id.to_string(),
                source,
            })
        })
}

fn ensure_event_matches_stream(
    expected_stream_id: &str,
    event: &JobEvent,
) -> Result<(), CronError> {
    if event.stream_id() == expected_stream_id {
        Ok(())
    } else {
        Err(CronError::event_source(
            "event payload stream id does not match the expected stream",
            std::io::Error::other(format!(
                "expected '{}' but event carried '{}'",
                expected_stream_id,
                event.stream_id()
            )),
        ))
    }
}

fn validate_event_job_id(id: &str) -> Result<(), SubjectTokenViolation> {
    trogon_nats::NatsToken::new(id).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CronJob, DeliverySpec, JobAdded, JobDetails, JobEnabledState, JobEventState, JobId,
        JobPaused, JobRemoved, JobSpec, MessageContent, MessageHeaders, ScheduleSpec,
    };

    fn job_id(id: &str) -> JobId {
        JobId::parse(id).unwrap()
    }

    fn job(id: &str) -> JobSpec {
        JobSpec {
            id: job_id(id),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::Every { every_sec: 30 },
            delivery: DeliverySpec::nats_event("agent.run").unwrap(),
            content: MessageContent::from_static(br#"{"kind":"heartbeat"}"#),
            headers: MessageHeaders::default(),
        }
    }

    fn expected_job(id: &str) -> CronJob {
        CronJob::from((id.to_string(), JobDetails::from(job(id))))
    }

    #[test]
    fn event_projection_replays_latest_state() {
        let events = [
            JobEvent::JobAdded(JobAdded {
                id: "backup".to_string(),
                job: job("backup").into(),
            }),
            JobEvent::JobPaused(JobPaused {
                id: "backup".to_string(),
            }),
            JobEvent::JobRemoved(JobRemoved {
                id: "backup".to_string(),
            }),
        ];
        let mut state = initial_state();

        for event in events {
            state = apply(state, event).unwrap();
        }

        assert_eq!(state, JobStreamState::Deleted("backup".to_string()));
    }

    #[test]
    fn event_projection_rejects_recreating_deleted_job() {
        let error = apply(
            JobStreamState::Deleted("backup".to_string()),
            JobEvent::JobAdded(JobAdded {
                id: "backup".to_string(),
                job: job("backup").into(),
            }),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            JobTransitionError::CannotAddDeletedJob { .. }
        ));
    }

    #[test]
    fn state_change_requires_existing_job() {
        let error = apply(
            initial_state(),
            JobEvent::JobPaused(JobPaused {
                id: "missing".to_string(),
            }),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            JobTransitionError::MissingJobForStateChange { .. }
        ));
    }

    #[test]
    fn projection_change_tracks_latest_state() {
        let before = initial_state();
        let after = apply(
            before.clone(),
            JobEvent::JobAdded(JobAdded {
                id: "backup".to_string(),
                job: job("backup").into(),
            }),
        )
        .unwrap();
        assert_eq!(
            projection_change(&before, &after),
            Some(ProjectionChange::Upsert(expected_job("backup")))
        );

        let updated = apply(
            after.clone(),
            JobEvent::JobPaused(JobPaused {
                id: "backup".to_string(),
            }),
        )
        .unwrap();
        match projection_change(&after, &updated).unwrap() {
            ProjectionChange::Upsert(job) => assert_eq!(job.state, JobEventState::Disabled),
            ProjectionChange::Delete(_) => panic!("expected upsert change"),
        }
    }

    #[test]
    fn initial_state_rejects_adding_existing_job() {
        let error = apply(
            JobStreamState::Present(expected_job("backup")),
            JobEvent::JobAdded(JobAdded {
                id: "backup".to_string(),
                job: job("backup").into(),
            }),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            JobTransitionError::CannotAddExistingJob { .. }
        ));
    }

    #[test]
    fn initial_removal_creates_deleted_tombstone() {
        let state = apply(
            initial_state(),
            JobEvent::JobRemoved(JobRemoved {
                id: "backup".to_string(),
            }),
        )
        .unwrap();
        assert_eq!(state, JobStreamState::Deleted("backup".to_string()));
    }

    #[test]
    fn reducer_rejects_stream_id_mismatch() {
        let stream_id = "alpha".to_string();
        let error = ensure_event_matches_stream(
            &stream_id,
            &JobEvent::JobRemoved(JobRemoved {
                id: "beta".to_string(),
            }),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("event payload stream id does not match")
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
    fn snapshot_projection_rejects_recreating_deleted_job() {
        let mut states = BTreeMap::new();
        let mut snapshots = BTreeMap::new();
        let stream_id = "alpha".to_string();

        apply_event_to_snapshot_map(
            &mut states,
            &mut snapshots,
            &stream_id,
            &JobEvent::JobAdded(JobAdded {
                id: "alpha".to_string(),
                job: JobDetails::from(job("alpha")),
            }),
            1,
        )
        .unwrap();
        apply_event_to_snapshot_map(
            &mut states,
            &mut snapshots,
            &stream_id,
            &JobEvent::JobPaused(JobPaused {
                id: "alpha".to_string(),
            }),
            2,
        )
        .unwrap();
        apply_event_to_snapshot_map(
            &mut states,
            &mut snapshots,
            &stream_id,
            &JobEvent::JobRemoved(JobRemoved {
                id: "alpha".to_string(),
            }),
            3,
        )
        .unwrap();
        let error = apply_event_to_snapshot_map(
            &mut states,
            &mut snapshots,
            &stream_id,
            &JobEvent::JobAdded(JobAdded {
                id: "alpha".to_string(),
                job: JobDetails::from(job("alpha")),
            }),
            4,
        )
        .unwrap_err();

        assert!(error.to_string().contains("deleted"));
        assert!(!snapshots.contains_key("alpha"));
        assert_eq!(
            states.get("alpha"),
            Some(&JobStreamState::Deleted(stream_id))
        );
    }

    #[test]
    fn snapshot_projection_rejects_invalid_transition_sequence() {
        let stream_id = "alpha".to_string();
        let error = apply_event_to_snapshot_map(
            &mut BTreeMap::new(),
            &mut BTreeMap::new(),
            &stream_id,
            &JobEvent::JobPaused(JobPaused {
                id: "alpha".to_string(),
            }),
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
