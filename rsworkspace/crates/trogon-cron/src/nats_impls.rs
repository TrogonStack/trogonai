use std::collections::{BTreeMap, HashSet};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_nats::jetstream::{
    self,
    consumer::{AckPolicy, DeliverPolicy, ReplayPolicy, pull},
    context::PublishErrorKind,
    context::traits::Publisher as _,
    kv,
    message::PublishMessage,
};
use futures::{Stream, StreamExt, TryStreamExt};
use trogon_nats::NatsToken;
use uuid::Uuid;

use crate::{
    config::{JobEnabledState, JobSpec, JobWriteCondition, JobWriteState, VersionedJobSpec},
    domain::{ResolvedJobSpec, validate_job_spec},
    error::{CronError, JobSpecError},
    events::{JobEvent, ProjectionChange, RecordedJobEvent, apply_event_to_versioned_state},
    kv::{
        CONFIG_BUCKET, EVENTS_STREAM, EVENTS_SUBJECT_PATTERN, EVENTS_SUBJECT_PREFIX,
        JOBS_KEY_PREFIX, LEGACY_EVENTS_SUBJECT_PATTERN, LEGACY_EVENTS_SUBJECT_PREFIX,
        SCHEDULES_STREAM, SNAPSHOT_BUCKET, SNAPSHOT_KEY_PREFIX, SNAPSHOT_LAST_EVENT_SEQUENCE_KEY,
        get_or_create_config_bucket, get_or_create_events_stream, get_or_create_schedule_stream,
        get_or_create_snapshot_bucket,
    },
    traits::{ConfigStore, JobSpecChange, SchedulePublisher},
};

const NATS_BATCH_COMMIT: &str = "Nats-Batch-Commit";
const NATS_BATCH_ID: &str = "Nats-Batch-Id";
const NATS_BATCH_SEQUENCE: &str = "Nats-Batch-Sequence";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventSubjectPrefix {
    Canonical,
    Legacy,
}

impl EventSubjectPrefix {
    fn as_str(self) -> &'static str {
        match self {
            Self::Canonical => EVENTS_SUBJECT_PREFIX,
            Self::Legacy => LEGACY_EVENTS_SUBJECT_PREFIX,
        }
    }

    fn subject(self, job_id: &str) -> String {
        format!("{}{}", self.as_str(), job_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AggregateSubjectState {
    prefix: EventSubjectPrefix,
    write_state: JobWriteState,
}

enum SnapshotChange {
    Upsert(Box<VersionedJobSpec>),
    Delete(String),
}

#[derive(Clone)]
pub struct NatsConfigStore {
    js: jetstream::Context,
}

impl NatsConfigStore {
    pub async fn new(nats: async_nats::Client) -> Result<Self, CronError> {
        let js = jetstream::new(nats.clone());
        get_or_create_config_bucket(&js).await?;
        get_or_create_snapshot_bucket(&js).await?;
        validate_events_stream(&get_or_create_events_stream(&js).await?)?;
        let store = Self { js };
        store.catch_up_snapshots().await?;
        Ok(store)
    }

    async fn config_bucket(&self) -> Result<kv::Store, CronError> {
        self.js
            .get_key_value(CONFIG_BUCKET)
            .await
            .map_err(|source| CronError::kv_source("failed to open config bucket", source))
    }

    async fn snapshot_bucket(&self) -> Result<kv::Store, CronError> {
        self.js
            .get_key_value(SNAPSHOT_BUCKET)
            .await
            .map_err(|source| CronError::kv_source("failed to open snapshot bucket", source))
    }

    async fn events_stream(&self) -> Result<jetstream::stream::Stream, CronError> {
        self.js
            .get_stream(EVENTS_STREAM)
            .await
            .map_err(|source| CronError::event_source("failed to open events stream", source))
    }

    async fn current_subject_state(
        &self,
        subject: &str,
    ) -> Result<Option<JobWriteState>, CronError> {
        let stream = self.events_stream().await?;
        match stream.get_last_raw_message_by_subject(subject).await {
            Ok(message) => {
                let version = message.sequence;
                let event = RecordedJobEvent::from_stream_message(message)?;
                let exists = !matches!(event.event, JobEvent::JobRemoved { .. });
                Ok(Some(JobWriteState::new(Some(version), exists)))
            }
            Err(error)
                if matches!(
                    error.kind(),
                    async_nats::jetstream::stream::LastRawMessageErrorKind::NoMessageFound
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(CronError::event_source(
                "failed to read latest aggregate version",
                error,
            )),
        }
    }

    async fn aggregate_subject_state(
        &self,
        job_id: &str,
    ) -> Result<AggregateSubjectState, CronError> {
        let canonical_version = self
            .current_subject_state(&EventSubjectPrefix::Canonical.subject(job_id))
            .await?;
        let legacy_version = self
            .current_subject_state(&EventSubjectPrefix::Legacy.subject(job_id))
            .await?;

        resolve_event_subject_state(job_id, canonical_version, legacy_version)
    }

    async fn append_event(
        &self,
        job_id: &str,
        write_condition: JobWriteCondition,
        event: RecordedJobEvent,
    ) -> Result<(), CronError> {
        let aggregate = self.aggregate_subject_state(job_id).await?;
        write_condition.ensure(job_id, aggregate.write_state)?;
        let expected_version = aggregate.write_state.current_version().unwrap_or(0);
        let batch_id = Uuid::new_v4().to_string();
        let payload = serde_json::to_vec(&event)?;
        let publish = PublishMessage::build()
            .payload(payload.into())
            .message_id(&event.event_id)
            .expected_last_subject_sequence(expected_version)
            .header(NATS_BATCH_ID, batch_id.as_str())
            .header(NATS_BATCH_SEQUENCE, "1")
            .header(NATS_BATCH_COMMIT, "1");
        let ack = self
            .js
            .publish_message(
                publish.outbound_message(event.subject_with_prefix(aggregate.prefix.as_str())),
            )
            .await
            .map_err(|source| CronError::event_source("failed to publish job event", source))?;

        match ack.await {
            Ok(_) => {}
            Err(error) if error.kind() == PublishErrorKind::WrongLastSequence => {
                return Err(CronError::OptimisticConcurrencyConflict {
                    id: job_id.to_string(),
                    expected: write_condition,
                    current_version: self
                        .aggregate_subject_state(job_id)
                        .await?
                        .write_state
                        .current_version(),
                });
            }
            Err(error) => {
                return Err(CronError::event_source(
                    "failed to acknowledge job event batch",
                    error,
                ));
            }
        }

        self.project_event_to_snapshot(job_id, &event).await?;

        Ok(())
    }

    async fn load_snapshot(&self, id: &str) -> Result<Option<VersionedJobSpec>, CronError> {
        let bucket = self.snapshot_bucket().await?;
        let Some(entry) = bucket.entry(snapshot_key(id)).await.map_err(|source| {
            CronError::kv_source("failed to read aggregate snapshot entry", source)
        })?
        else {
            return Ok(None);
        };

        serde_json::from_slice::<VersionedJobSpec>(&entry.value)
            .map(Some)
            .map_err(|source| {
                CronError::kv_source("failed to decode aggregate snapshot entry", source)
            })
    }

    async fn list_snapshots(&self) -> Result<Vec<VersionedJobSpec>, CronError> {
        let bucket = self.snapshot_bucket().await?;
        let mut keys = bucket.keys().await.map_err(|source| {
            CronError::kv_source("failed to list aggregate snapshot keys", source)
        })?;
        let mut jobs = Vec::new();

        while let Some(result) = keys.next().await {
            let key = result.map_err(|source| {
                CronError::kv_source("failed to read aggregate snapshot key", source)
            })?;
            if !key.starts_with(SNAPSHOT_KEY_PREFIX) {
                continue;
            }
            let Some(entry) = bucket.entry(key).await.map_err(|source| {
                CronError::kv_source("failed to read aggregate snapshot value", source)
            })?
            else {
                continue;
            };
            let job =
                serde_json::from_slice::<VersionedJobSpec>(&entry.value).map_err(|source| {
                    CronError::kv_source("failed to decode aggregate snapshot value", source)
                })?;
            jobs.push(job);
        }

        Ok(jobs)
    }

    async fn catch_up_snapshots(&self) -> Result<(), CronError> {
        let stream = self.events_stream().await?;
        let info = stream.get_info().await.map_err(|source| {
            CronError::event_source(
                "failed to query events stream info for snapshot catch-up",
                source,
            )
        })?;
        if info.state.messages == 0 {
            return Ok(());
        }

        let bucket = self.snapshot_bucket().await?;
        let checkpoint = read_snapshot_checkpoint(&bucket).await?;
        if checkpoint >= info.state.last_sequence {
            return Ok(());
        }

        let mut snapshots = load_snapshot_map(&bucket).await?;
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
            let event = RecordedJobEvent::from_stream_message(message)?;
            let change = apply_event_to_snapshot_map(&mut snapshots, &event.event, sequence)?;
            persist_snapshot_change(&bucket, change).await?;
            write_snapshot_checkpoint(&bucket, sequence).await?;
        }

        Ok(())
    }

    async fn project_event_to_snapshot(
        &self,
        job_id: &str,
        event: &RecordedJobEvent,
    ) -> Result<(), CronError> {
        let bucket = self.snapshot_bucket().await?;
        let mut snapshots = BTreeMap::new();
        if let Some(snapshot) = self.load_snapshot(job_id).await? {
            snapshots.insert(job_id.to_string(), snapshot);
        }

        let aggregate = self.aggregate_subject_state(job_id).await?;
        let final_version = aggregate.write_state.current_version().ok_or_else(|| {
            CronError::event_source(
                "aggregate snapshot projection requires an event version",
                std::io::Error::other(format!("job '{job_id}'")),
            )
        })?;

        let change = apply_event_to_snapshot_map(&mut snapshots, &event.event, final_version)?;
        persist_snapshot_change(&bucket, change).await?;
        maybe_advance_snapshot_checkpoint(&bucket, final_version).await
    }

    async fn rewrite_projection(&self, jobs: &[VersionedJobSpec]) -> Result<(), CronError> {
        let kv = self.config_bucket().await?;
        let mut keys = kv
            .keys()
            .await
            .map_err(|source| CronError::kv_source("failed to list projection keys", source))?;

        while let Some(result) = keys.next().await {
            let key = result
                .map_err(|source| CronError::kv_source("failed to read projection key", source))?;
            if key.starts_with(JOBS_KEY_PREFIX) {
                let _ = kv.purge(key).await;
            }
        }

        for job in jobs {
            let key = format!("{JOBS_KEY_PREFIX}{}", job.id());
            let value = serde_json::to_vec(&job.spec)?;
            kv.put(key, value.into()).await.map_err(|source| {
                CronError::kv_source("failed to write projected job state", source)
            })?;
        }

        Ok(())
    }
}

impl ConfigStore for NatsConfigStore {
    async fn put_job(
        &self,
        config: &JobSpec,
        write_condition: JobWriteCondition,
    ) -> Result<(), CronError> {
        validate_job_spec(config)?;
        self.append_event(
            &config.id,
            write_condition,
            RecordedJobEvent::new(JobEvent::job_registered(config.clone())),
        )
        .await
    }

    async fn set_job_state(
        &self,
        id: &str,
        state: JobEnabledState,
        write_condition: JobWriteCondition,
    ) -> Result<(), CronError> {
        validate_job_id(id)?;
        self.get_job(id)
            .await?
            .ok_or_else(|| CronError::JobNotFound { id: id.to_string() })?;
        self.append_event(
            id,
            write_condition,
            RecordedJobEvent::new(JobEvent::job_state_changed(id, state)),
        )
        .await
    }

    async fn get_job(&self, id: &str) -> Result<Option<VersionedJobSpec>, CronError> {
        validate_job_id(id)?;
        self.load_snapshot(id).await
    }

    async fn delete_job(
        &self,
        id: &str,
        write_condition: JobWriteCondition,
    ) -> Result<(), CronError> {
        validate_job_id(id)?;
        self.get_job(id)
            .await?
            .ok_or_else(|| CronError::JobNotFound { id: id.to_string() })?;
        self.append_event(
            id,
            write_condition,
            RecordedJobEvent::new(JobEvent::job_removed(id)),
        )
        .await
    }

    async fn list_jobs(&self) -> Result<Vec<VersionedJobSpec>, CronError> {
        self.list_snapshots().await
    }

    async fn load_and_watch(
        &self,
    ) -> Result<
        (
            Vec<JobSpec>,
            Pin<Box<dyn Stream<Item = JobSpecChange> + Send + 'static>>,
        ),
        CronError,
    > {
        let stream = self.events_stream().await?;
        let info = stream.get_info().await.map_err(|source| {
            CronError::event_source("failed to query events stream info", source)
        })?;
        let last_sequence = info.state.last_sequence;
        let initial_jobs =
            rebuild_jobs_from_stream(&stream, info.state.first_sequence, last_sequence).await?;
        self.rewrite_projection(&initial_jobs).await?;
        let consumer = stream
            .create_consumer(event_watch_consumer_config(next_watch_start_sequence(
                last_sequence,
            )))
            .await
            .map_err(|source| {
                CronError::event_source("failed to create job event watch consumer", source)
            })?;
        let subscriber = consumer.messages().await.map_err(|source| {
            CronError::event_source("failed to open job event watch stream", source)
        })?;

        let kv = self.config_bucket().await?;
        let state = initial_jobs
            .iter()
            .cloned()
            .map(|job| (job.id().to_string(), job.spec))
            .collect::<BTreeMap<_, _>>();
        let state = Arc::new(Mutex::new(state));
        let stream = subscriber
            .then(move |result| {
                let state = Arc::clone(&state);
                let kv = kv.clone();
                async move {
                    let message = match result {
                        Ok(message) => message,
                        Err(error) => {
                            tracing::error!(error = %error, "Failed to read job event from watch consumer");
                            return None;
                        }
                    };

                    let event = match serde_json::from_slice::<RecordedJobEvent>(
                        &message.message.payload,
                    ) {
                        Ok(event) => event,
                        Err(error) => {
                            tracing::error!(error = %error, "Failed to decode job event from subscription");
                            ack_watch_message(&message).await;
                            return None;
                        }
                    };

                    let projection_change = {
                        let mut state = state.lock().expect("job event state mutex poisoned");
                        event.event.apply_to_state(&mut state)
                    };
                    let projection_change = match projection_change {
                        Ok(change) => change,
                        Err(error) => {
                            tracing::error!(error = %error, "Failed to apply job event to current state");
                            ack_watch_message(&message).await;
                            return None;
                        }
                    };

                    if let Err(error) = apply_projection_change(&kv, &projection_change).await {
                        tracing::error!(error = %error, "Failed to update projected job state from event");
                        ack_watch_message(&message).await;
                        return None;
                    }

                    ack_watch_message(&message).await;
                    Some(change_from_projection_change(projection_change))
                }
            })
            .filter_map(futures::future::ready);

        Ok((
            initial_jobs.into_iter().map(|job| job.spec).collect(),
            Box::pin(stream),
        ))
    }
}

async fn rebuild_jobs_from_stream(
    stream: &jetstream::stream::Stream,
    first_sequence: u64,
    last_sequence: u64,
) -> Result<Vec<VersionedJobSpec>, CronError> {
    let mut jobs = BTreeMap::new();
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
        let event = RecordedJobEvent::from_stream_message(message)?;
        apply_event_to_versioned_state(&mut jobs, &event.event, version)?;
    }

    Ok(jobs.into_values().collect())
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

fn change_from_projection_change(change: ProjectionChange) -> JobSpecChange {
    match change {
        ProjectionChange::Upsert(job) => JobSpecChange::Put(job),
        ProjectionChange::Delete(id) => JobSpecChange::Delete(id),
    }
}

fn validate_job_id(id: &str) -> Result<(), CronError> {
    NatsToken::new(id).map(|_| ()).map_err(|source| {
        CronError::invalid_job_spec(JobSpecError::InvalidId {
            id: id.to_string(),
            source,
        })
    })
}

fn snapshot_key(id: &str) -> String {
    format!("{SNAPSHOT_KEY_PREFIX}{id}")
}

fn checkpoint_value(sequence: u64) -> Vec<u8> {
    sequence.to_string().into_bytes()
}

async fn read_snapshot_checkpoint_entry(
    bucket: &kv::Store,
) -> Result<(Option<u64>, u64), CronError> {
    let Some(entry) = bucket
        .entry(SNAPSHOT_LAST_EVENT_SEQUENCE_KEY)
        .await
        .map_err(|source| {
            CronError::kv_source("failed to read aggregate snapshot checkpoint entry", source)
        })?
    else {
        return Ok((None, 0));
    };

    let sequence = String::from_utf8(entry.value.to_vec())
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            CronError::kv_source(
                "failed to decode aggregate snapshot checkpoint",
                std::io::Error::other(SNAPSHOT_LAST_EVENT_SEQUENCE_KEY),
            )
        })?;

    Ok((Some(entry.revision), sequence))
}

async fn read_snapshot_checkpoint(bucket: &kv::Store) -> Result<u64, CronError> {
    let (_revision, sequence) = read_snapshot_checkpoint_entry(bucket).await?;
    Ok(sequence)
}

async fn write_snapshot_checkpoint(bucket: &kv::Store, sequence: u64) -> Result<(), CronError> {
    write_kv_value(
        bucket,
        SNAPSHOT_LAST_EVENT_SEQUENCE_KEY,
        checkpoint_value(sequence),
    )
    .await
}

async fn maybe_advance_snapshot_checkpoint(
    bucket: &kv::Store,
    sequence: u64,
) -> Result<(), CronError> {
    let expected_previous = sequence.saturating_sub(1);
    let (revision, current_sequence) = read_snapshot_checkpoint_entry(bucket).await?;
    if current_sequence != expected_previous {
        return Ok(());
    }

    let value = checkpoint_value(sequence);
    match revision {
        Some(revision) => match bucket
            .update(SNAPSHOT_LAST_EVENT_SEQUENCE_KEY, value.into(), revision)
            .await
        {
            Ok(_) => Ok(()),
            Err(source) if source.kind() == kv::UpdateErrorKind::WrongLastRevision => Ok(()),
            Err(source) => Err(CronError::kv_source(
                "failed to advance aggregate snapshot checkpoint",
                source,
            )),
        },
        None => match bucket
            .create(SNAPSHOT_LAST_EVENT_SEQUENCE_KEY, value.into())
            .await
        {
            Ok(_) => Ok(()),
            Err(source) if source.kind() == kv::CreateErrorKind::AlreadyExists => Ok(()),
            Err(source) => Err(CronError::kv_source(
                "failed to create aggregate snapshot checkpoint",
                source,
            )),
        },
    }
}

async fn load_snapshot_map(
    bucket: &kv::Store,
) -> Result<BTreeMap<String, VersionedJobSpec>, CronError> {
    let mut keys = bucket
        .keys()
        .await
        .map_err(|source| CronError::kv_source("failed to list aggregate snapshot keys", source))?;
    let mut snapshots = BTreeMap::new();

    while let Some(result) = keys.next().await {
        let key = result.map_err(|source| {
            CronError::kv_source("failed to read aggregate snapshot key", source)
        })?;
        if !key.starts_with(SNAPSHOT_KEY_PREFIX) {
            continue;
        }
        let Some(value) = bucket.get(key.clone()).await.map_err(|source| {
            CronError::kv_source("failed to read aggregate snapshot value", source)
        })?
        else {
            continue;
        };
        let snapshot = serde_json::from_slice::<VersionedJobSpec>(&value).map_err(|source| {
            CronError::kv_source("failed to decode aggregate snapshot value", source)
        })?;
        snapshots.insert(snapshot.id().to_string(), snapshot);
    }

    Ok(snapshots)
}

fn apply_event_to_snapshot_map(
    snapshots: &mut BTreeMap<String, VersionedJobSpec>,
    event: &JobEvent,
    version: u64,
) -> Result<SnapshotChange, CronError> {
    apply_event_to_versioned_state(snapshots, event, version)?;
    match event {
        JobEvent::JobRegistered { spec } => Ok(SnapshotChange::Upsert(Box::new(
            snapshots
                .get(&spec.id)
                .cloned()
                .expect("registered job snapshot must exist"),
        ))),
        JobEvent::JobStateChanged { id, .. } => Ok(SnapshotChange::Upsert(Box::new(
            snapshots
                .get(id)
                .cloned()
                .expect("state-changed job snapshot must exist"),
        ))),
        JobEvent::JobRemoved { id } => Ok(SnapshotChange::Delete(id.clone())),
    }
}

async fn persist_snapshot_change(
    bucket: &kv::Store,
    change: SnapshotChange,
) -> Result<(), CronError> {
    match change {
        SnapshotChange::Upsert(snapshot) => {
            let value = serde_json::to_vec(snapshot.as_ref())?;
            write_kv_value(bucket, &snapshot_key(snapshot.id()), value).await?;
        }
        SnapshotChange::Delete(id) => {
            delete_kv_value(bucket, &snapshot_key(&id)).await?;
        }
    }

    Ok(())
}

async fn write_kv_value(bucket: &kv::Store, key: &str, value: Vec<u8>) -> Result<(), CronError> {
    if let Some(entry) = bucket.entry(key.to_string()).await.map_err(|source| {
        CronError::kv_source("failed to read key-value entry for update", source)
    })? {
        let _ = bucket
            .update(key, value.into(), entry.revision)
            .await
            .map_err(|source| CronError::kv_source("failed to update key-value entry", source))?;
    } else {
        let _ = bucket
            .create(key, value.into())
            .await
            .map_err(|source| CronError::kv_source("failed to create key-value entry", source))?;
    }

    Ok(())
}

async fn delete_kv_value(bucket: &kv::Store, key: &str) -> Result<(), CronError> {
    if let Some(entry) = bucket.entry(key.to_string()).await.map_err(|source| {
        CronError::kv_source("failed to read key-value entry for delete", source)
    })? {
        bucket
            .delete_expect_revision(key, Some(entry.revision))
            .await
            .map_err(|source| CronError::kv_source("failed to delete key-value entry", source))?;
    }

    Ok(())
}

fn resolve_event_subject_state(
    job_id: &str,
    canonical_state: Option<JobWriteState>,
    legacy_state: Option<JobWriteState>,
) -> Result<AggregateSubjectState, CronError> {
    match (canonical_state, legacy_state) {
        (Some(_), Some(_)) => Err(CronError::event_source(
            "job aggregate has conflicting event subjects",
            std::io::Error::other(format!("job '{job_id}'")),
        )),
        (Some(write_state), None) => Ok(AggregateSubjectState {
            prefix: EventSubjectPrefix::Canonical,
            write_state,
        }),
        (None, Some(write_state)) => Ok(AggregateSubjectState {
            prefix: EventSubjectPrefix::Legacy,
            write_state,
        }),
        (None, None) => Ok(AggregateSubjectState {
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
        let mut config = stream.cached_info().config.clone();
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

fn validate_events_stream(stream: &jetstream::stream::Stream) -> Result<(), CronError> {
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
    use crate::config::{DeliverySpec, ScheduleSpec};

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
        let mut state = BTreeMap::from([("alpha".to_string(), test_job("alpha"))]);

        let change = JobEvent::job_state_changed("alpha", JobEnabledState::Disabled)
            .apply_to_state(&mut state)
            .unwrap();

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
    fn new_aggregates_use_canonical_event_subject() {
        let state = resolve_event_subject_state("alpha", None, None).unwrap();

        assert_eq!(state.prefix, EventSubjectPrefix::Canonical);
        assert_eq!(state.write_state.current_version(), None);
        assert!(!state.write_state.exists());
    }

    #[test]
    fn legacy_aggregates_keep_legacy_event_subject() {
        let state =
            resolve_event_subject_state("alpha", None, Some(JobWriteState::new(Some(12), true)))
                .unwrap();

        assert_eq!(state.prefix, EventSubjectPrefix::Legacy);
        assert_eq!(state.write_state.current_version(), Some(12));
        assert!(state.write_state.exists());
    }

    #[test]
    fn deleted_aggregates_can_be_recreated_without_changing_subject() {
        let state =
            resolve_event_subject_state("alpha", Some(JobWriteState::new(Some(12), false)), None)
                .unwrap();

        assert_eq!(state.prefix, EventSubjectPrefix::Canonical);
        assert_eq!(state.write_state.current_version(), Some(12));
        assert!(!state.write_state.exists());
    }

    #[test]
    fn split_aggregate_subjects_are_rejected() {
        let error = resolve_event_subject_state(
            "alpha",
            Some(JobWriteState::new(Some(7), true)),
            Some(JobWriteState::new(Some(9), true)),
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("job aggregate has conflicting event subjects")
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
}
