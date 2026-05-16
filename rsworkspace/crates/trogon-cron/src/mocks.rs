use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use async_nats::jetstream::kv;
use buffa::MessageField;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::{Serialize, de::DeserializeOwned};
use trogon_eventsourcing::snapshot::Snapshot;
use trogon_eventsourcing::{
    AppendStreamRequest, AppendStreamResponse, Event, ReadSnapshotRequest, ReadSnapshotResponse, ReadStreamRequest,
    ReadStreamResponse, SnapshotRead, SnapshotWrite, StreamAppend, StreamPosition, StreamRead, StreamWritePrecondition,
    WriteSnapshotRequest, WriteSnapshotResponse,
};
use trogon_nats::lease::{ReleaseLease, RenewLease, TryAcquireLease};

use crate::{
    GetJobCommand, JobEventCase, JobEventCodec, ListJobsCommand, ResolvedJob,
    config::{JobWriteCondition, JobWriteState},
    error::CronError,
    projections::{CronJobWatchStream, LoadAndWatchCronJobsResult},
    read_model::{
        CronJob, JobEventDelivery, JobEventSamplingSource, JobEventSchedule, JobEventStatus, MessageContent,
        MessageEnvelope, MessageHeaders,
    },
    traits::SchedulePublisher,
    v1,
};

#[derive(Clone, Default)]
pub struct MockSchedulePublisher {
    upserts: Arc<Mutex<Vec<String>>>,
    removals: Arc<Mutex<Vec<String>>>,
    active: Arc<Mutex<HashSet<String>>>,
}

impl MockSchedulePublisher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upserts(&self) -> Vec<String> {
        self.upserts.lock().unwrap().clone()
    }

    pub fn removals(&self) -> Vec<String> {
        self.removals.lock().unwrap().clone()
    }

    pub fn seed_active_job(&self, job_id: &str) {
        self.active.lock().unwrap().insert(job_id.to_string());
    }
}

impl SchedulePublisher for MockSchedulePublisher {
    type Error = CronError;

    async fn active_schedule_ids(&self) -> Result<HashSet<String>, Self::Error> {
        Ok(self.active.lock().unwrap().clone())
    }

    async fn upsert_schedule(&self, job: &ResolvedJob) -> Result<(), Self::Error> {
        self.upserts.lock().unwrap().push(job.schedule_subject().to_string());
        self.active.lock().unwrap().insert(job.id().to_string());
        Ok(())
    }

    async fn remove_schedule(&self, job_id: &str) -> Result<(), Self::Error> {
        self.removals.lock().unwrap().push(job_id.to_string());
        self.active.lock().unwrap().remove(job_id);
        Ok(())
    }
}

#[derive(Clone)]
pub struct MockLeaderLock {
    allow_acquire: Arc<AtomicBool>,
    allow_renew: Arc<AtomicBool>,
    next_revision: Arc<AtomicU64>,
}

impl Default for MockLeaderLock {
    fn default() -> Self {
        Self {
            allow_acquire: Arc::new(AtomicBool::new(true)),
            allow_renew: Arc::new(AtomicBool::new(true)),
            next_revision: Arc::new(AtomicU64::new(1)),
        }
    }
}

impl MockLeaderLock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_allow_acquire(&self, allowed: bool) {
        self.allow_acquire.store(allowed, Ordering::SeqCst);
    }

    pub fn set_allow_renew(&self, allowed: bool) {
        self.allow_renew.store(allowed, Ordering::SeqCst);
    }
}

impl TryAcquireLease for MockLeaderLock {
    type Error = kv::CreateError;

    async fn try_acquire(&self, _value: Bytes) -> Result<u64, Self::Error> {
        if self.allow_acquire.load(Ordering::SeqCst) {
            Ok(self.next_revision.fetch_add(1, Ordering::SeqCst))
        } else {
            Err(kv::CreateError::new(kv::CreateErrorKind::AlreadyExists))
        }
    }
}

impl RenewLease for MockLeaderLock {
    type Error = kv::UpdateError;

    async fn renew(&self, _value: Bytes, revision: u64) -> Result<u64, Self::Error> {
        if self.allow_renew.load(Ordering::SeqCst) {
            Ok(revision + 1)
        } else {
            Err(kv::UpdateError::new(kv::UpdateErrorKind::Other))
        }
    }
}

impl ReleaseLease for MockLeaderLock {
    type Error = kv::DeleteError;

    async fn release(&self, _revision: u64) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct MockCronStore {
    jobs: Arc<Mutex<HashMap<String, CronJob>>>,
    stream_positions: Arc<Mutex<HashMap<String, StreamPosition>>>,
    events: Arc<Mutex<HashMap<String, Vec<Event>>>>,
    command_snapshots: Arc<Mutex<HashMap<String, HashMap<String, String>>>>,
}

fn stream_position(value: u64) -> Result<StreamPosition, CronError> {
    StreamPosition::try_new(value)
        .map_err(|source| CronError::event_source("mock stream position must be non-zero", source))
}

impl MockCronStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seed_job(&self, job: CronJob) {
        let id = job.id.clone();
        let event = v1::JobEvent {
            event: Some(
                v1::JobAdded {
                    job: MessageField::some(cron_job_to_proto_details(&job)),
                }
                .into(),
            ),
        };

        let initial_position = StreamPosition::new(NonZeroU64::MIN);
        self.stream_positions
            .lock()
            .unwrap()
            .insert(id.clone(), initial_position);
        self.events.lock().unwrap().insert(
            id.clone(),
            vec![Event::from_domain_event(&JobEventCodec, &event).unwrap()],
        );
        self.jobs.lock().unwrap().insert(id.clone(), job);
    }

    pub(crate) fn read_command_snapshot<Payload>(
        &self,
        config: trogon_eventsourcing::SnapshotStoreConfig,
        stream_id: &(impl AsRef<str> + ?Sized),
    ) -> Result<Option<Snapshot<Payload>>, CronError>
    where
        Payload: DeserializeOwned,
    {
        self.command_snapshots
            .lock()
            .unwrap()
            .get(config.key_prefix())
            .and_then(|snapshots| snapshots.get(stream_id.as_ref()).cloned())
            .map(|snapshot| serde_json::from_str(&snapshot))
            .transpose()
            .map_err(CronError::from)
    }

    pub async fn get_job(&self, command: GetJobCommand) -> Result<Option<CronJob>, CronError> {
        Ok(self.jobs.lock().unwrap().get(command.id.as_str()).cloned())
    }

    pub async fn list_jobs(&self, _command: ListJobsCommand) -> Result<Vec<CronJob>, CronError> {
        Ok(self.jobs.lock().unwrap().values().cloned().collect())
    }

    pub async fn load_and_watch_cron_jobs(&self) -> LoadAndWatchCronJobsResult {
        let jobs = self.jobs.lock().unwrap().values().cloned().collect();
        Ok((jobs, Box::pin(futures::stream::pending()) as CronJobWatchStream))
    }
}

fn cron_job_to_proto_details(job: &CronJob) -> v1::JobDetails {
    v1::JobDetails {
        status: match job.status {
            JobEventStatus::Enabled => v1::JobStatus::JOB_STATUS_ENABLED,
            JobEventStatus::Disabled => v1::JobStatus::JOB_STATUS_DISABLED,
        },
        schedule: MessageField::some(proto_schedule(&job.schedule)),
        delivery: MessageField::some(proto_delivery(&job.delivery)),
        message: MessageField::some(proto_message(&job.message)),
    }
}

fn proto_schedule(schedule: &JobEventSchedule) -> v1::JobSchedule {
    let kind = match schedule {
        JobEventSchedule::At { at } => v1::AtSchedule { at: at.clone() }.into(),
        JobEventSchedule::Every { every_sec } => v1::EverySchedule { every_sec: *every_sec }.into(),
        JobEventSchedule::Cron { expr, timezone } => v1::CronSchedule {
            expr: expr.clone(),
            timezone: timezone.clone().unwrap_or_default(),
        }
        .into(),
    };
    v1::JobSchedule { kind: Some(kind) }
}

fn proto_delivery(delivery: &JobEventDelivery) -> v1::JobDelivery {
    match delivery {
        JobEventDelivery::NatsEvent { route, ttl_sec, source } => v1::JobDelivery {
            kind: Some(
                v1::NatsEventDelivery {
                    route: route.clone(),
                    ttl_sec: *ttl_sec,
                    source: source
                        .as_ref()
                        .map(proto_sampling_source)
                        .map(MessageField::some)
                        .unwrap_or_else(MessageField::none),
                }
                .into(),
            ),
        },
    }
}

fn proto_sampling_source(source: &JobEventSamplingSource) -> v1::JobSamplingSource {
    match source {
        JobEventSamplingSource::LatestFromSubject { subject } => v1::JobSamplingSource {
            kind: Some(
                v1::LatestFromSubjectSampling {
                    subject: subject.clone(),
                }
                .into(),
            ),
        },
    }
}

fn proto_message(message: &MessageEnvelope) -> v1::JobMessage {
    v1::JobMessage {
        content: message.content.as_str().to_string(),
        headers: message
            .headers
            .as_slice()
            .iter()
            .map(|(name, value)| v1::Header {
                name: name.clone(),
                value: value.clone(),
            })
            .collect(),
    }
}

fn cron_job_from_proto(stream_id: &str, details: &v1::JobDetails) -> CronJob {
    CronJob {
        id: stream_id.to_string(),
        status: if details.status == v1::JobStatus::JOB_STATUS_DISABLED {
            JobEventStatus::Disabled
        } else {
            JobEventStatus::Enabled
        },
        schedule: details
            .schedule
            .as_option()
            .map(schedule_from_proto)
            .unwrap_or(JobEventSchedule::Every { every_sec: 0 }),
        delivery: details
            .delivery
            .as_option()
            .map(delivery_from_proto)
            .unwrap_or_else(|| JobEventDelivery::NatsEvent {
                route: String::new(),
                ttl_sec: None,
                source: None,
            }),
        message: details
            .message
            .as_option()
            .map(message_from_proto)
            .unwrap_or_else(|| MessageEnvelope {
                content: MessageContent::new(String::new()),
                headers: MessageHeaders::default(),
            }),
    }
}

fn schedule_from_proto(schedule: &v1::JobSchedule) -> JobEventSchedule {
    match schedule.kind.as_ref() {
        Some(v1::__buffa::oneof::job_schedule::Kind::At(inner)) => JobEventSchedule::At { at: inner.at.clone() },
        Some(v1::__buffa::oneof::job_schedule::Kind::Every(inner)) => JobEventSchedule::Every {
            every_sec: inner.every_sec,
        },
        Some(v1::__buffa::oneof::job_schedule::Kind::Cron(inner)) => JobEventSchedule::Cron {
            expr: inner.expr.clone(),
            timezone: (!inner.timezone.is_empty()).then(|| inner.timezone.clone()),
        },
        None => JobEventSchedule::Every { every_sec: 0 },
    }
}

fn delivery_from_proto(delivery: &v1::JobDelivery) -> JobEventDelivery {
    match delivery.kind.as_ref() {
        Some(v1::__buffa::oneof::job_delivery::Kind::NatsEvent(inner)) => JobEventDelivery::NatsEvent {
            route: inner.route.clone(),
            ttl_sec: inner.ttl_sec,
            source: inner.source.as_option().map(sampling_source_from_proto),
        },
        None => JobEventDelivery::NatsEvent {
            route: String::new(),
            ttl_sec: None,
            source: None,
        },
    }
}

fn sampling_source_from_proto(source: &v1::JobSamplingSource) -> JobEventSamplingSource {
    match source.kind.as_ref() {
        Some(v1::__buffa::oneof::job_sampling_source::Kind::LatestFromSubject(inner)) => {
            JobEventSamplingSource::LatestFromSubject {
                subject: inner.subject.clone(),
            }
        }
        None => JobEventSamplingSource::LatestFromSubject { subject: String::new() },
    }
}

fn message_from_proto(message: &v1::JobMessage) -> MessageEnvelope {
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

impl StreamRead<str> for MockCronStore {
    type Error = CronError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        let stream_id = request.stream_id;
        let from_sequence = request.from_sequence;
        let current_position = self.stream_positions.lock().unwrap().get(stream_id).copied();
        let stream_events = self.events.lock().unwrap().get(stream_id).cloned().unwrap_or_default();
        if from_sequence == 0 {
            return Ok(ReadStreamResponse {
                current_position,
                events: Vec::new(),
            });
        }

        let mut recorded = Vec::new();
        for (index, event) in stream_events.into_iter().enumerate() {
            let sequence = index as u64 + 1;
            if sequence < from_sequence {
                continue;
            }
            recorded.push(event.record(
                stream_id,
                stream_position(sequence)?,
                DateTime::<Utc>::from_timestamp(1_700_000_000 + sequence as i64, 0).ok_or_else(|| {
                    CronError::event_source(
                        "failed to build mocked recorded event timestamp",
                        std::io::Error::other(stream_id.to_string()),
                    )
                })?,
            ));
        }
        Ok(ReadStreamResponse {
            current_position,
            events: recorded,
        })
    }
}

impl StreamAppend<str> for MockCronStore {
    type Error = CronError;

    async fn append_stream(&self, request: AppendStreamRequest<'_, str>) -> Result<AppendStreamResponse, Self::Error> {
        let stream_id = request.stream_id.to_string();
        let expected_state = request.stream_write_precondition;
        let events = request.events;
        let jobs = self.jobs.clone();
        let stream_positions = self.stream_positions.clone();
        let event_log = self.events.clone();

        let mut jobs = jobs.lock().unwrap();
        let mut stream_positions = stream_positions.lock().unwrap();
        let mut stream_events = event_log.lock().unwrap();

        let current_job = jobs.get(stream_id.as_str()).cloned();
        let current_position = stream_positions.get(stream_id.as_str()).copied();
        let write_state = JobWriteState::new(current_position, current_position.is_some());
        match expected_state {
            StreamWritePrecondition::Any => {}
            StreamWritePrecondition::StreamExists if write_state.exists() => {}
            StreamWritePrecondition::StreamExists => {
                return Err(CronError::OptimisticConcurrencyConflict {
                    id: stream_id.to_string(),
                    expected: StreamWritePrecondition::StreamExists,
                    current_position,
                });
            }
            StreamWritePrecondition::NoStream => {
                JobWriteCondition::MustNotExist.ensure(stream_id.as_str(), write_state)?;
            }
            StreamWritePrecondition::At(position) => {
                JobWriteCondition::MustBeAtPosition(position).ensure(stream_id.as_str(), write_state)?;
            }
        }

        let stored_events = stream_events.entry(stream_id.to_string()).or_default();
        let mut projected_job = current_job;
        let mut raw_position = current_position.map(StreamPosition::get).unwrap_or(0);

        for event_data in events {
            let event = event_data
                .decode_with(stream_id.as_str(), &JobEventCodec)
                .map_err(|source| CronError::event_source("failed to decode mocked job event payload", source))?;
            raw_position += 1;
            stored_events.push(event_data);
            match &event.event {
                Some(JobEventCase::JobAdded(inner)) => {
                    let details = inner.job.as_option().ok_or_else(|| {
                        CronError::event_source(
                            "failed to project mocked job add without job details",
                            std::io::Error::other(stream_id.to_string()),
                        )
                    })?;
                    projected_job = Some(cron_job_from_proto(stream_id.as_str(), details));
                }
                Some(JobEventCase::JobPaused(_)) => {
                    let mut job = projected_job.take().ok_or_else(|| {
                        CronError::event_source(
                            "failed to project mocked job pause without current read model",
                            std::io::Error::other(stream_id.to_string()),
                        )
                    })?;
                    job.status = crate::JobEventStatus::Disabled;
                    projected_job = Some(job);
                }
                Some(JobEventCase::JobResumed(_)) => {
                    let mut job = projected_job.take().ok_or_else(|| {
                        CronError::event_source(
                            "failed to project mocked job resume without current read model",
                            std::io::Error::other(stream_id.to_string()),
                        )
                    })?;
                    job.status = crate::JobEventStatus::Enabled;
                    projected_job = Some(job);
                }
                Some(JobEventCase::JobRemoved(_)) => {
                    projected_job = None;
                }
                None => {
                    return Err(CronError::event_source(
                        "failed to project mocked job event without supported case",
                        std::io::Error::other("missing event case"),
                    ));
                }
            }
        }

        let final_position = stream_position(raw_position)?;
        stream_positions.insert(stream_id.to_string(), final_position);
        if let Some(job) = projected_job {
            jobs.insert(stream_id.to_string(), job);
        } else {
            jobs.remove(stream_id.as_str());
        }
        Ok(AppendStreamResponse {
            stream_position: final_position,
        })
    }
}

impl<Payload> SnapshotRead<Payload, str> for MockCronStore
where
    Payload: Serialize + DeserializeOwned + Send,
{
    type Error = CronError;

    async fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, str>,
    ) -> Result<ReadSnapshotResponse<Payload>, Self::Error> {
        self.read_command_snapshot(request.config, request.stream_id)
            .map(ReadSnapshotResponse::new)
    }
}

impl<Payload> SnapshotWrite<Payload, str> for MockCronStore
where
    Payload: Serialize + DeserializeOwned + Send,
{
    type Error = CronError;

    async fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, Payload, str>,
    ) -> Result<WriteSnapshotResponse, Self::Error> {
        let snapshot = serde_json::to_string(&request.snapshot)?;
        self.command_snapshots
            .lock()
            .unwrap()
            .entry(request.config.key_prefix().to_string())
            .or_default()
            .insert(request.stream_id.to_string(), snapshot);
        Ok(WriteSnapshotResponse::new())
    }
}

#[cfg(test)]
mod tests {
    use trogon_eventsourcing::{CommandExecution, CommandFailure, run_task_immediately};

    use super::*;
    use crate::commands::domain as command_domain;
    use crate::{
        AddJobCommand, CronJob, GetJobCommand, JobEventDelivery, JobEventSchedule, JobEventStatus, JobId,
        JobWriteCondition, ListJobsCommand, MessageContent, MessageEnvelope, MessageHeaders, PauseJobCommand,
        RemoveJobCommand, ResumeJobCommand,
    };

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).expect("test stream position must be non-zero")
    }
    use futures::StreamExt;

    fn command_job_id(id: &str) -> command_domain::JobId {
        command_domain::JobId::parse(id).unwrap()
    }

    fn base_job(id: &str) -> CronJob {
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

    fn command_base_job(id: &str) -> command_domain::Job {
        command_domain::Job {
            id: command_job_id(id),
            status: command_domain::JobStatus::Enabled,
            schedule: command_domain::Schedule::every(30).unwrap(),
            delivery: command_domain::Delivery::nats_event("agent.run").unwrap(),
            message: command_domain::JobMessage {
                content: command_domain::MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
                headers: command_domain::JobHeaders::default(),
            },
        }
    }

    fn expected_job(id: &str) -> CronJob {
        base_job(id)
    }

    #[tokio::test]
    async fn mock_schedule_publisher_tracks_active_jobs() {
        let publisher = MockSchedulePublisher::new();
        publisher.seed_active_job("orphan");
        let details = cron_job_to_proto_details(&expected_job("alpha"));
        let resolved = ResolvedJob::from_event("alpha", &details).unwrap();

        let active = publisher.active_schedule_ids().await.unwrap();
        assert!(active.contains("orphan"));

        publisher.upsert_schedule(&resolved).await.unwrap();
        publisher.remove_schedule("orphan").await.unwrap();

        assert_eq!(publisher.upserts(), vec!["cron.schedules.alpha"]);
        assert_eq!(publisher.removals(), vec!["orphan"]);
        assert!(publisher.active_schedule_ids().await.unwrap().contains("alpha"));
    }

    #[tokio::test]
    async fn mock_leader_lock_covers_success_and_failure_paths() {
        let lock = MockLeaderLock::new();

        let first = lock.try_acquire(Bytes::new()).await.unwrap();
        assert_eq!(first, 1);
        assert_eq!(lock.renew(Bytes::new(), first).await.unwrap(), 2);
        lock.release(first).await.unwrap();

        lock.allow_acquire.store(false, Ordering::SeqCst);
        assert_eq!(
            lock.try_acquire(Bytes::new()).await.unwrap_err().kind(),
            kv::CreateErrorKind::AlreadyExists
        );

        lock.allow_renew.store(false, Ordering::SeqCst);
        assert_eq!(
            lock.renew(Bytes::new(), 2).await.unwrap_err().kind(),
            kv::UpdateErrorKind::Other
        );
    }

    #[tokio::test]
    async fn mock_cron_store_covers_crud_and_read_model_watch() {
        let store = MockCronStore::new();
        store.seed_job(base_job("seeded"));

        let seeded = store
            .get_job(GetJobCommand::new(JobId::parse("seeded").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(seeded, expected_job("seeded"));

        CommandExecution::new(&store, &AddJobCommand::new(command_base_job("alpha")))
            .with_snapshot(&store)
            .with_task_runtime(run_task_immediately)
            .execute()
            .await
            .unwrap();
        let alpha = store
            .get_job(GetJobCommand::new(JobId::parse("alpha").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(alpha, expected_job("alpha"));

        CommandExecution::new(&store, &PauseJobCommand::new(command_job_id("alpha")))
            .with_snapshot(&store)
            .with_task_runtime(run_task_immediately)
            .execute()
            .await
            .unwrap();
        assert_eq!(
            store
                .get_job(GetJobCommand::new(JobId::parse("alpha").unwrap()))
                .await
                .unwrap()
                .unwrap()
                .status,
            JobEventStatus::Disabled
        );

        let listed = store.list_jobs(ListJobsCommand).await.unwrap();
        assert_eq!(listed.len(), 2);

        let (watch_jobs, mut watcher) = store.load_and_watch_cron_jobs().await.unwrap();
        assert_eq!(watch_jobs.len(), 2);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(5), watcher.next())
                .await
                .is_err()
        );

        CommandExecution::new(&store, &RemoveJobCommand::new(command_job_id("alpha")))
            .with_snapshot(&store)
            .with_task_runtime(run_task_immediately)
            .execute()
            .await
            .unwrap();
        assert!(
            store
                .get_job(GetJobCommand::new(JobId::parse("alpha").unwrap()))
                .await
                .unwrap()
                .is_none()
        );

        let deleted_error = CommandExecution::new(&store, &AddJobCommand::new(command_base_job("alpha")))
            .with_snapshot(&store)
            .with_task_runtime(run_task_immediately)
            .execute()
            .await
            .unwrap_err();
        assert!(matches!(
            deleted_error,
            CommandFailure::Decide(crate::AddJobDecideError::JobDeleted { .. })
        ));
    }

    #[tokio::test]
    async fn mock_cron_store_rejects_invalid_specs_and_state_errors() {
        let store = MockCronStore::new();
        let invalid_error = serde_json::from_value::<command_domain::Job>(serde_json::json!({
            "id": "bad",
            "schedule": { "type": "every", "every_sec": 30 },
            "delivery": {
                "type": "nats_event",
                "route": "agent.run",
                "source": { "type": "latest_from_subject", "subject": "sensors.>" }
            },
            "content": "{\"kind\":\"heartbeat\"}"
        }))
        .unwrap_err();
        assert!(invalid_error.to_string().contains("sampling source"));

        CommandExecution::new(&store, &AddJobCommand::new(command_base_job("alpha")))
            .with_snapshot(&store)
            .with_task_runtime(run_task_immediately)
            .execute()
            .await
            .unwrap();
        let same_state_error = CommandExecution::new(&store, &ResumeJobCommand::new(command_job_id("alpha")))
            .with_snapshot(&store)
            .with_task_runtime(run_task_immediately)
            .execute()
            .await
            .unwrap_err();
        assert!(matches!(
            same_state_error,
            CommandFailure::Decide(crate::ResumeJobDecideError::AlreadyActive { .. })
        ));

        let missing_error = CommandExecution::new(&store, &PauseJobCommand::new(command_job_id("missing")))
            .with_snapshot(&store)
            .with_task_runtime(run_task_immediately)
            .execute()
            .await
            .unwrap_err();
        assert!(matches!(
            missing_error,
            CommandFailure::Decide(crate::PauseJobDecideError::JobNotFound { .. })
        ));
    }

    #[test]
    fn ensure_write_condition_covers_accept_and_conflict_paths() {
        JobWriteCondition::MustNotExist
            .ensure("alpha", JobWriteState::new(None, false))
            .unwrap();
        JobWriteCondition::MustBeAtPosition(position(3))
            .ensure("alpha", JobWriteState::new(Some(position(3)), true))
            .unwrap();

        let error = JobWriteCondition::MustNotExist
            .ensure("alpha", JobWriteState::new(Some(position(4)), true))
            .unwrap_err();
        assert!(matches!(
            error,
            CronError::OptimisticConcurrencyConflict {
                current_position: Some(_),
                ..
            }
        ));

        let error = JobWriteCondition::MustBeAtPosition(position(3))
            .ensure("alpha", JobWriteState::new(Some(position(4)), true))
            .unwrap_err();
        assert!(matches!(
            error,
            CronError::OptimisticConcurrencyConflict {
                current_position: Some(_),
                ..
            }
        ));
    }
}
