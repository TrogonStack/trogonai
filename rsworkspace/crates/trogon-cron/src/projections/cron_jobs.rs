use std::collections::BTreeMap;
use std::pin::Pin;
use std::time::Duration;

use async_nats::jetstream::{
    self,
    consumer::{AckPolicy, DeliverPolicy, ReplayPolicy, pull},
    kv,
};
use futures::{Stream, StreamExt};
use trogon_eventsourcing::{EventData, RecordedEvent, StreamPosition, record_stream_message};
use trogon_nats::SubjectTokenViolation;
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream};

use crate::{
    error::CronError,
    kv::{CRON_JOBS_CHECKPOINT_KEY, EVENTS_SUBJECT_PREFIX},
    proto::{JobEventCodec, v1},
    read_model::{
        CronJob, JobEventDelivery, JobEventSamplingSource, JobEventSchedule, JobEventStatus, MessageContent,
        MessageEnvelope, MessageHeaders,
    },
    store::{open_cron_jobs_bucket, open_events_stream},
};

pub type CronJobWatchStream = Pin<Box<dyn Stream<Item = CronJobChange> + Send + 'static>>;
pub type LoadAndWatchCronJobsResult = Result<(Vec<CronJob>, CronJobWatchStream), CronError>;

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

#[derive(Debug, Clone, PartialEq)]
struct WatchedProjectionChange {
    stream_id: String,
    next_state: JobStreamState,
    change: Option<ProjectionChange>,
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
    InvalidEventId { id: String, source: SubjectTokenViolation },
    MalformedEvent { context: &'static str },
    CannotAddExistingJob { id: String },
    CannotAddDeletedJob { id: String },
    MissingJobForStateChange { id: String },
    DeletedJobForStateChange { id: String },
    DeletedJobForRemoval { id: String },
}

pub const fn initial_state() -> JobStreamState {
    JobStreamState::Initial
}

pub fn apply(
    stream_id: &str,
    state: JobStreamState,
    event: &v1::JobEvent,
) -> Result<JobStreamState, JobTransitionError> {
    validate_event_job_id(stream_id).map_err(|source| JobTransitionError::InvalidEventId {
        id: stream_id.to_string(),
        source,
    })?;

    match (state, &event.event) {
        (JobStreamState::Initial, Some(v1::__buffa::oneof::job_event::Event::JobAdded(inner))) => {
            let job = inner.job.as_option().ok_or(JobTransitionError::MalformedEvent {
                context: "job added event has no job details",
            })?;
            Ok(JobStreamState::Present(project_job(stream_id, job)?))
        }
        (JobStreamState::Initial, Some(v1::__buffa::oneof::job_event::Event::JobPaused(_))) => {
            Err(JobTransitionError::MissingJobForStateChange {
                id: stream_id.to_string(),
            })
        }
        (JobStreamState::Initial, Some(v1::__buffa::oneof::job_event::Event::JobResumed(_))) => {
            Err(JobTransitionError::MissingJobForStateChange {
                id: stream_id.to_string(),
            })
        }
        (JobStreamState::Initial, Some(v1::__buffa::oneof::job_event::Event::JobRemoved(_))) => {
            Ok(JobStreamState::Deleted(stream_id.to_string()))
        }
        (JobStreamState::Present(job), Some(v1::__buffa::oneof::job_event::Event::JobAdded(_))) => {
            Err(JobTransitionError::CannotAddExistingJob { id: job.id })
        }
        (JobStreamState::Present(mut job), Some(v1::__buffa::oneof::job_event::Event::JobPaused(_))) => {
            job.status = JobEventStatus::Disabled;
            Ok(JobStreamState::Present(job))
        }
        (JobStreamState::Present(mut job), Some(v1::__buffa::oneof::job_event::Event::JobResumed(_))) => {
            job.status = JobEventStatus::Enabled;
            Ok(JobStreamState::Present(job))
        }
        (JobStreamState::Present(job), Some(v1::__buffa::oneof::job_event::Event::JobRemoved(_))) => {
            Ok(JobStreamState::Deleted(job.id))
        }
        (JobStreamState::Deleted(id), Some(v1::__buffa::oneof::job_event::Event::JobAdded(_))) => {
            Err(JobTransitionError::CannotAddDeletedJob { id })
        }
        (JobStreamState::Deleted(id), Some(v1::__buffa::oneof::job_event::Event::JobPaused(_))) => {
            Err(JobTransitionError::DeletedJobForStateChange { id })
        }
        (JobStreamState::Deleted(id), Some(v1::__buffa::oneof::job_event::Event::JobResumed(_))) => {
            Err(JobTransitionError::DeletedJobForStateChange { id })
        }
        (JobStreamState::Deleted(id), Some(v1::__buffa::oneof::job_event::Event::JobRemoved(_))) => {
            Err(JobTransitionError::DeletedJobForRemoval { id })
        }
        (_, None) => Err(JobTransitionError::MalformedEvent {
            context: "job event has no supported case",
        }),
    }
}

fn project_job(stream_id: &str, job: &v1::JobDetails) -> Result<CronJob, JobTransitionError> {
    let schedule = job.schedule.as_option().ok_or(JobTransitionError::MalformedEvent {
        context: "job details has no schedule",
    })?;
    let delivery = job.delivery.as_option().ok_or(JobTransitionError::MalformedEvent {
        context: "job details has no delivery",
    })?;
    let message = job.message.as_option().ok_or(JobTransitionError::MalformedEvent {
        context: "job details has no message",
    })?;
    Ok(CronJob {
        id: stream_id.to_string(),
        status: project_status(job.status),
        schedule: project_schedule(schedule)?,
        delivery: project_delivery(delivery)?,
        message: project_message(message),
    })
}

fn project_status(status: v1::JobStatus) -> JobEventStatus {
    if status == v1::JobStatus::JOB_STATUS_DISABLED {
        JobEventStatus::Disabled
    } else {
        JobEventStatus::Enabled
    }
}

fn project_schedule(schedule: &v1::JobSchedule) -> Result<JobEventSchedule, JobTransitionError> {
    match schedule.kind.as_ref() {
        Some(v1::__buffa::oneof::job_schedule::Kind::At(inner)) => Ok(JobEventSchedule::At { at: inner.at.clone() }),
        Some(v1::__buffa::oneof::job_schedule::Kind::Every(inner)) => Ok(JobEventSchedule::Every {
            every_sec: inner.every_sec,
        }),
        Some(v1::__buffa::oneof::job_schedule::Kind::Cron(inner)) => Ok(JobEventSchedule::Cron {
            expr: inner.expr.clone(),
            timezone: (!inner.timezone.is_empty()).then(|| inner.timezone.clone()),
        }),
        None => Err(JobTransitionError::MalformedEvent {
            context: "job schedule has no supported case",
        }),
    }
}

fn project_delivery(delivery: &v1::JobDelivery) -> Result<JobEventDelivery, JobTransitionError> {
    match delivery.kind.as_ref() {
        Some(v1::__buffa::oneof::job_delivery::Kind::NatsEvent(inner)) => Ok(JobEventDelivery::NatsEvent {
            route: inner.route.clone(),
            ttl_sec: inner.ttl_sec,
            source: inner.source.as_option().map(project_sampling_source).transpose()?,
        }),
        None => Err(JobTransitionError::MalformedEvent {
            context: "job delivery has no supported case",
        }),
    }
}

fn project_sampling_source(source: &v1::JobSamplingSource) -> Result<JobEventSamplingSource, JobTransitionError> {
    match source.kind.as_ref() {
        Some(v1::__buffa::oneof::job_sampling_source::Kind::LatestFromSubject(inner)) => {
            Ok(JobEventSamplingSource::LatestFromSubject {
                subject: inner.subject.clone(),
            })
        }
        None => Err(JobTransitionError::MalformedEvent {
            context: "job sampling source has no supported case",
        }),
    }
}

fn project_message(message: &v1::JobMessage) -> MessageEnvelope {
    MessageEnvelope {
        content: MessageContent::new(message.content.clone()),
        headers: MessageHeaders::from_pairs(
            message
                .headers
                .iter()
                .map(|header| (header.name.clone(), header.value.clone())),
        ),
    }
}

pub fn projection_change(before: &JobStreamState, after: &JobStreamState) -> Option<ProjectionChange> {
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
}

impl From<CronJob> for JobStreamState {
    fn from(job: CronJob) -> Self {
        Self::Present(job)
    }
}

impl std::fmt::Display for JobTransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEventId { id, .. } => write!(f, "job event id '{id}' is invalid"),
            Self::MalformedEvent { context } => write!(f, "job event is malformed: {context}"),
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
            Self::InvalidEventId { source, .. } => Some(source),
            Self::MalformedEvent { .. }
            | Self::CannotAddExistingJob { .. }
            | Self::CannotAddDeletedJob { .. }
            | Self::MissingJobForStateChange { .. }
            | Self::DeletedJobForStateChange { .. }
            | Self::DeletedJobForRemoval { .. } => None,
        }
    }
}

pub async fn load_and_watch_cron_jobs<J>(js: &J) -> LoadAndWatchCronJobsResult
where
    J: JetStreamGetKeyValue<Store = kv::Store> + JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    let stream: jetstream::stream::Stream = open_events_stream(js).await?;
    let info = stream
        .get_info()
        .await
        .map_err(|source| CronError::event_source("failed to query events stream info", source))?;
    let last_sequence = info.state.last_sequence;
    let initial_jobs = rebuild_jobs_from_stream(&stream, info.state.first_sequence, last_sequence).await?;
    rewrite_cron_jobs_projection(js, &initial_jobs).await?;
    let consumer = stream
        .create_consumer(event_watch_consumer_config(next_watch_start_sequence(last_sequence)))
        .await
        .map_err(|source| CronError::event_source("failed to create cron job event watch consumer", source))?;
    let subscriber = consumer
        .messages()
        .await
        .map_err(|source| CronError::event_source("failed to open cron job event watch stream", source))?;

    let kv: kv::Store = open_cron_jobs_bucket(js).await?;
    let state = initial_jobs
        .iter()
        .cloned()
        .map(|job| (job.id.to_string(), JobStreamState::Present(job)))
        .collect::<BTreeMap<_, _>>();
    let watcher: CronJobWatchStream = Box::pin(futures::stream::unfold(
        (state, subscriber, kv),
        |(mut state, mut subscriber, kv)| async move {
            loop {
                let result = subscriber.next().await?;
                let message = match result {
                    Ok(message) => message,
                    Err(error) => {
                        tracing::error!(error = %error, "Failed to read cron job event from watch consumer");
                        continue;
                    }
                };
                let Some(projection_change) = prepare_watched_projection_change(&state, &message) else {
                    ack_watch_message(&message).await;
                    continue;
                };

                let WatchedProjectionChange {
                    stream_id,
                    next_state,
                    change,
                } = projection_change;

                if let Some(change) = change.as_ref()
                    && let Err(error) = apply_projection_change(&kv, change).await
                {
                    tracing::error!(error = %error, "Failed to update projected cron jobs state from event");
                    nak_watch_message(&message).await;
                    continue;
                }

                commit_watched_projection_state(&mut state, stream_id, next_state);
                ack_watch_message(&message).await;
                if let Some(change) = change {
                    return Some((change_from_projection_change(change), (state, subscriber, kv)));
                }
            }
        },
    ));

    Ok((initial_jobs, watcher))
}

pub(crate) async fn catch_up_cron_jobs_read_model<J>(js: &J) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store> + JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    let stream: jetstream::stream::Stream = open_events_stream(js).await?;
    let info = stream.get_info().await.map_err(|source| {
        CronError::event_source(
            "failed to query events stream info for cron jobs read-model catch-up",
            source,
        )
    })?;
    if info.state.messages == 0 {
        return Ok(());
    }

    let bucket = open_cron_jobs_bucket(js).await?;
    let checkpoint = read_read_model_checkpoint(&bucket).await?;
    if checkpoint >= info.state.last_sequence {
        return Ok(());
    }

    let mut states = read_model_state_map(&bucket).await?;
    let start = checkpoint.max(info.state.first_sequence.saturating_sub(1)) + 1;

    let consumer = stream
        .create_consumer(event_replay_consumer_config(start))
        .await
        .map_err(|source| CronError::event_source("failed to create cron jobs read-model catch-up consumer", source))?;
    let mut messages = consumer
        .messages()
        .await
        .map_err(|source| CronError::event_source("failed to open cron jobs read-model catch-up stream", source))?;

    while let Some(message) = messages.next().await {
        let message = message.map_err(|source| {
            CronError::event_source("failed to read job event during cron jobs read-model catch-up", source)
        })?;
        let sequence = event_message_sequence(&message, "failed to read cron jobs read-model catch-up event metadata")?;
        if sequence > info.state.last_sequence {
            break;
        }
        let reached_tail = sequence >= info.state.last_sequence;
        let event = decode_recorded_watch_message(&message)?;
        let stream_id = job_id_from_event_subject(event.stream_id())?;
        let data = event.decode_data_with(&JobEventCodec).map_err(|source| {
            CronError::event_source(
                "failed to decode job event during cron jobs read-model catch-up",
                source,
            )
        })?;
        if let Some(change) = apply_event_to_read_model_state(&mut states, &stream_id, &data)? {
            apply_projection_change(&bucket, &change).await?;
        }
        write_read_model_checkpoint(&bucket, sequence).await?;
        if reached_tail {
            break;
        }
    }

    Ok(())
}

pub(crate) async fn project_appended_events(
    bucket: &kv::Store,
    job_id: &str,
    events: &[EventData],
    final_position: StreamPosition,
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

    let mut states = BTreeMap::new();
    if let Some(job) = read_projected_job(bucket, job_id).await? {
        states.insert(job_id.to_string(), JobStreamState::from(job));
    }

    for event in events {
        let decoded = event
            .decode_data_with(&JobEventCodec)
            .map_err(|source| CronError::event_source("failed to decode job event for cron jobs read model", source))?;
        if let Some(change) = apply_event_to_read_model_state(&mut states, job_id, &decoded)? {
            apply_projection_change(bucket, &change).await?;
        }
    }
    maybe_advance_read_model_checkpoint(bucket, final_position.get()).await
}

async fn rewrite_cron_jobs_projection<J>(js: &J, jobs: &[CronJob]) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    let kv: kv::Store = open_cron_jobs_bucket(js).await?;
    let desired_ids = jobs
        .iter()
        .map(|job| job.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut keys = kv
        .keys()
        .await
        .map_err(|source| CronError::kv_source("failed to list projection keys", source))?;

    while let Some(result) = keys.next().await {
        let key = result.map_err(|source| CronError::kv_source("failed to read projection key", source))?;
        if is_read_model_metadata_key(&key) {
            continue;
        }
        if desired_ids.contains(key.as_str()) {
            continue;
        }
        kv.delete(key)
            .await
            .map_err(|source| CronError::kv_source("failed to delete stale projected job state", source))?;
    }

    for job in jobs {
        let value = serde_json::to_vec(job)?;
        kv.put(job.id.to_string(), value.into())
            .await
            .map_err(|source| CronError::kv_source("failed to write projected job state", source))?;
    }

    Ok(())
}

async fn rebuild_jobs_from_stream(
    stream: &jetstream::stream::Stream,
    first_sequence: u64,
    last_sequence: u64,
) -> Result<Vec<CronJob>, CronError> {
    let mut states = BTreeMap::new();
    if last_sequence == 0 || first_sequence == 0 || first_sequence > last_sequence {
        return Ok(Vec::new());
    }

    let consumer = stream
        .create_consumer(event_replay_consumer_config(first_sequence))
        .await
        .map_err(|source| CronError::event_source("failed to create cron job projection replay consumer", source))?;
    let mut messages = consumer
        .messages()
        .await
        .map_err(|source| CronError::event_source("failed to open cron job projection replay stream", source))?;

    while let Some(message) = messages.next().await {
        let message =
            message.map_err(|source| CronError::event_source("failed to read job event from stream", source))?;
        let sequence = event_message_sequence(&message, "failed to read job event metadata")?;
        if sequence > last_sequence {
            break;
        }
        let reached_tail = sequence >= last_sequence;
        let event = decode_recorded_watch_message(&message)?;
        let stream_id = job_id_from_event_subject(event.stream_id())?;
        let data = event
            .decode_data_with(&JobEventCodec)
            .map_err(|source| CronError::event_source("failed to decode recorded job event payload", source))?;
        apply_event_to_read_model_state(&mut states, &stream_id, &data)?;
        if reached_tail {
            break;
        }
    }

    Ok(states.into_values().filter_map(JobStreamState::into_job).collect())
}

fn decode_recorded_job_event(
    message: async_nats::jetstream::message::StreamMessage,
) -> Result<RecordedEvent, CronError> {
    record_stream_message(message)
        .map_err(|source| CronError::event_source("failed to decode stored job event", source))
}

fn decode_recorded_watch_message(message: &async_nats::jetstream::Message) -> Result<RecordedEvent, CronError> {
    let stream_message =
        async_nats::jetstream::message::StreamMessage::try_from(message.message.clone()).map_err(|source| {
            CronError::event_source("failed to reconstruct stream message from watch delivery", source)
        })?;

    decode_recorded_job_event(stream_message)
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

fn event_replay_consumer_config(start_sequence: u64) -> pull::OrderedConfig {
    pull::OrderedConfig {
        deliver_policy: DeliverPolicy::ByStartSequence { start_sequence },
        replay_policy: ReplayPolicy::Instant,
        ..Default::default()
    }
}

fn prepare_watched_projection_change(
    state: &BTreeMap<String, JobStreamState>,
    message: &jetstream::Message,
) -> Option<WatchedProjectionChange> {
    let event = match decode_recorded_watch_message(message) {
        Ok(event) => event,
        Err(error) => {
            tracing::error!(error = %error, "Failed to decode cron job event from watcher");
            return None;
        }
    };

    let stream_id = match job_id_from_event_subject(event.stream_id()) {
        Ok(stream_id) => stream_id,
        Err(error) => {
            tracing::error!(error = %error, "Failed to derive watched cron job stream id from subject");
            return None;
        }
    };

    let data = match event.decode_data_with(&JobEventCodec) {
        Ok(data) => data,
        Err(error) => {
            tracing::error!(error = %error, "Failed to decode watched cron job event payload");
            return None;
        }
    };

    prepare_projection_change(state, stream_id.as_str(), &data)
}

fn prepare_projection_change(
    state: &BTreeMap<String, JobStreamState>,
    stream_id: &str,
    event: &v1::JobEvent,
) -> Option<WatchedProjectionChange> {
    let current = state.get(stream_id).cloned().unwrap_or_else(initial_state);
    let next = match apply(stream_id, current.clone(), event)
        .map_err(|error| CronError::event_source("failed to apply watched job event to stream state", error))
    {
        Ok(next) => next,
        Err(error) => {
            tracing::error!(error = %error, "Failed to apply job event to current state");
            return None;
        }
    };
    let change = projection_change(&current, &next);

    Some(WatchedProjectionChange {
        stream_id: stream_id.to_string(),
        next_state: next,
        change,
    })
}

fn commit_watched_projection_state(
    state: &mut BTreeMap<String, JobStreamState>,
    stream_id: String,
    next: JobStreamState,
) {
    match next {
        JobStreamState::Present(_) | JobStreamState::Deleted(_) => {
            state.insert(stream_id, next);
        }
        JobStreamState::Initial => {
            state.remove(stream_id.as_str());
        }
    }
}

async fn ack_watch_message(message: &jetstream::Message) {
    if let Err(error) = message.ack().await {
        tracing::error!(error = %error, "Failed to acknowledge watched job event");
    }
}

async fn nak_watch_message(message: &jetstream::Message) {
    if let Err(error) = message.ack_with(jetstream::AckKind::Nak(None)).await {
        tracing::error!(error = %error, "Failed to negatively acknowledge watched job event");
    }
}

fn event_message_sequence(message: &jetstream::Message, context: &'static str) -> Result<u64, CronError> {
    message
        .info()
        .map(|info| info.stream_sequence)
        .map_err(|source| CronError::event_source(context, std::io::Error::other(source.to_string())))
}

fn is_read_model_metadata_key(key: &str) -> bool {
    key == CRON_JOBS_CHECKPOINT_KEY
}

async fn read_projected_job(bucket: &kv::Store, id: &str) -> Result<Option<CronJob>, CronError> {
    let Some(entry) = bucket
        .entry(id.to_string())
        .await
        .map_err(|source| CronError::kv_source("failed to read projected cron job", source))?
    else {
        return Ok(None);
    };

    serde_json::from_slice(&entry.value).map(Some).map_err(CronError::from)
}

async fn read_model_state_map(bucket: &kv::Store) -> Result<BTreeMap<String, JobStreamState>, CronError> {
    let mut keys = bucket
        .keys()
        .await
        .map_err(|source| CronError::kv_source("failed to list cron jobs read-model keys", source))?;
    let mut states = BTreeMap::new();

    while let Some(result) = keys.next().await {
        let key = result.map_err(|source| CronError::kv_source("failed to read cron jobs read-model key", source))?;
        if is_read_model_metadata_key(&key) {
            continue;
        }
        if let Some(job) = read_projected_job(bucket, &key).await? {
            states.insert(key, JobStreamState::Present(job));
        }
    }

    Ok(states)
}

async fn read_read_model_checkpoint(bucket: &kv::Store) -> Result<u64, CronError> {
    let Some(entry) = bucket
        .entry(CRON_JOBS_CHECKPOINT_KEY.to_string())
        .await
        .map_err(|source| CronError::kv_source("failed to read cron jobs read-model checkpoint", source))?
    else {
        return Ok(0);
    };

    String::from_utf8(entry.value.to_vec())
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            CronError::kv_source(
                "failed to decode cron jobs read-model checkpoint",
                std::io::Error::other(CRON_JOBS_CHECKPOINT_KEY),
            )
        })
}

async fn write_read_model_checkpoint(bucket: &kv::Store, sequence: u64) -> Result<(), CronError> {
    bucket
        .put(CRON_JOBS_CHECKPOINT_KEY.to_string(), sequence.to_string().into())
        .await
        .map(|_| ())
        .map_err(|source| CronError::kv_source("failed to write cron jobs read-model checkpoint", source))
}

async fn maybe_advance_read_model_checkpoint(bucket: &kv::Store, sequence: u64) -> Result<(), CronError> {
    let current = read_read_model_checkpoint(bucket).await?;
    if current != sequence.saturating_sub(1) {
        return Ok(());
    }

    write_read_model_checkpoint(bucket, sequence).await
}

async fn apply_projection_change(kv: &kv::Store, change: &ProjectionChange) -> Result<(), CronError> {
    match change {
        ProjectionChange::Upsert(job) => {
            let value = serde_json::to_vec(job)?;
            kv.put(job.id.to_string(), value.into())
                .await
                .map_err(|source| CronError::kv_source("failed to store projected job state", source))?;
        }
        ProjectionChange::Delete(id) => {
            kv.delete(id.clone())
                .await
                .map_err(|source| CronError::kv_source("failed to delete projected job state", source))?;
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

fn apply_event_to_read_model_state(
    states: &mut BTreeMap<String, JobStreamState>,
    stream_id: &str,
    event: &v1::JobEvent,
) -> Result<Option<ProjectionChange>, CronError> {
    let current_state = states.get(stream_id).cloned().unwrap_or_else(initial_state);
    let next_state = apply(stream_id, current_state.clone(), event)
        .map_err(|source| CronError::event_source("failed to apply job event to cron jobs read model", source))?;
    let change = projection_change(&current_state, &next_state);

    match next_state.clone() {
        JobStreamState::Present(_) | JobStreamState::Deleted(_) => {
            states.insert(stream_id.to_string(), next_state);
        }
        JobStreamState::Initial => {
            states.remove(stream_id);
        }
    }

    Ok(change)
}

fn job_id_from_event_subject(subject: &str) -> Result<String, CronError> {
    let raw_id = subject.strip_prefix(EVENTS_SUBJECT_PREFIX).ok_or_else(|| {
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

fn validate_event_job_id(id: &str) -> Result<(), SubjectTokenViolation> {
    trogon_nats::NatsToken::new(id).map(|_| ())
}

#[cfg(test)]
mod tests {
    use buffa::MessageField;

    use super::*;
    use crate::proto::v1;
    use crate::{
        CronJob, JobEventDelivery, JobEventSchedule, JobEventStatus, MessageContent, MessageEnvelope, MessageHeaders,
    };

    fn expected_job(id: &str) -> CronJob {
        CronJob {
            id: id.to_string(),
            status: JobEventStatus::Enabled,
            schedule: JobEventSchedule::Every { every_sec: 30 },
            delivery: JobEventDelivery::NatsEvent {
                route: "agent.run".to_string(),
                ttl_sec: None,
                source: None,
            },
            message: MessageEnvelope {
                content: MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
                headers: MessageHeaders::default(),
            },
        }
    }

    fn added_event(id: &str) -> v1::JobEvent {
        v1::JobEvent {
            event: Some(
                v1::JobAdded {
                    job: MessageField::some(proto_job_details(id)),
                }
                .into(),
            ),
        }
    }

    fn proto_job_details(_id: &str) -> v1::JobDetails {
        v1::JobDetails {
            status: v1::JobStatus::JOB_STATUS_ENABLED,
            schedule: MessageField::some(v1::JobSchedule {
                kind: Some(v1::EverySchedule { every_sec: 30 }.into()),
            }),
            delivery: MessageField::some(v1::JobDelivery {
                kind: Some(
                    v1::NatsEventDelivery {
                        route: "agent.run".to_string(),
                        ttl_sec: None,
                        source: MessageField::none(),
                    }
                    .into(),
                ),
            }),
            message: MessageField::some(v1::JobMessage {
                content: r#"{"kind":"heartbeat"}"#.to_string(),
                headers: Vec::new(),
            }),
        }
    }

    fn paused_event(_id: &str) -> v1::JobEvent {
        v1::JobEvent {
            event: Some(v1::JobPaused {}.into()),
        }
    }

    fn removed_event(_id: &str) -> v1::JobEvent {
        v1::JobEvent {
            event: Some(v1::JobRemoved {}.into()),
        }
    }

    #[test]
    fn event_projection_replays_latest_state() {
        let events = [added_event("backup"), paused_event("backup"), removed_event("backup")];
        let mut state = initial_state();

        for event in &events {
            state = apply("backup", state, event).unwrap();
        }

        assert_eq!(state, JobStreamState::Deleted("backup".to_string()));
    }

    #[test]
    fn event_projection_rejects_recreating_deleted_job() {
        let error = apply(
            "backup",
            JobStreamState::Deleted("backup".to_string()),
            &added_event("backup"),
        )
        .unwrap_err();

        assert!(matches!(error, JobTransitionError::CannotAddDeletedJob { .. }));
    }

    #[test]
    fn state_change_requires_existing_job() {
        let error = apply("missing", initial_state(), &paused_event("missing")).unwrap_err();

        assert!(matches!(error, JobTransitionError::MissingJobForStateChange { .. }));
    }

    #[test]
    fn projection_change_tracks_latest_state() {
        let before = initial_state();
        let after = apply("backup", before.clone(), &added_event("backup")).unwrap();
        assert_eq!(
            projection_change(&before, &after),
            Some(ProjectionChange::Upsert(expected_job("backup")))
        );

        let updated = apply("backup", after.clone(), &paused_event("backup")).unwrap();
        match projection_change(&after, &updated).unwrap() {
            ProjectionChange::Upsert(job) => assert_eq!(job.status, JobEventStatus::Disabled),
            ProjectionChange::Delete(_) => panic!("expected upsert change"),
        }
    }

    #[test]
    fn initial_state_rejects_adding_existing_job() {
        let error = apply(
            "backup",
            JobStreamState::Present(expected_job("backup")),
            &added_event("backup"),
        )
        .unwrap_err();
        assert!(matches!(error, JobTransitionError::CannotAddExistingJob { .. }));
    }

    #[test]
    fn watched_projection_change_does_not_mutate_state_before_commit() {
        let mut state = BTreeMap::new();
        let prepared = prepare_projection_change(&state, "backup", &added_event("backup")).unwrap();

        assert!(state.is_empty());
        assert_eq!(prepared.change, Some(ProjectionChange::Upsert(expected_job("backup"))));

        commit_watched_projection_state(&mut state, prepared.stream_id, prepared.next_state);

        assert!(matches!(state.get("backup"), Some(JobStreamState::Present(_))));
    }

    #[test]
    fn watched_projection_commits_tombstone_even_without_public_change() {
        let mut state = BTreeMap::new();
        let prepared = prepare_projection_change(&state, "backup", &removed_event("backup")).unwrap();

        assert!(prepared.change.is_none());

        commit_watched_projection_state(&mut state, prepared.stream_id, prepared.next_state);

        assert_eq!(
            state.get("backup"),
            Some(&JobStreamState::Deleted("backup".to_string()))
        );
    }

    #[test]
    fn initial_removal_creates_deleted_tombstone() {
        let state = apply("backup", initial_state(), &removed_event("backup")).unwrap();
        assert_eq!(state, JobStreamState::Deleted("backup".to_string()));
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
    fn read_model_state_rejects_recreating_deleted_job() {
        let mut states = BTreeMap::new();
        let stream_id = "alpha".to_string();

        apply_event_to_read_model_state(&mut states, &stream_id, &added_event("alpha")).unwrap();
        apply_event_to_read_model_state(&mut states, &stream_id, &paused_event("alpha")).unwrap();
        apply_event_to_read_model_state(&mut states, &stream_id, &removed_event("alpha")).unwrap();
        let error = apply_event_to_read_model_state(&mut states, &stream_id, &added_event("alpha")).unwrap_err();

        assert!(error.to_string().contains("deleted"));
        assert_eq!(states.get("alpha"), Some(&JobStreamState::Deleted(stream_id)));
    }

    #[test]
    fn read_model_state_rejects_invalid_transition_sequence() {
        let stream_id = "alpha".to_string();
        let error =
            apply_event_to_read_model_state(&mut BTreeMap::new(), &stream_id, &paused_event("alpha")).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("failed to apply job event to cron jobs read model")
        );
    }
}
