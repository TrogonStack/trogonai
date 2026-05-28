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
use trogon_decider_runtime::snapshot::Snapshot;
use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, Event, EventData, EventDecode, EventEncode, EventId, EventIdentity,
    EventType, Headers, ReadFrom, ReadSnapshotRequest, ReadSnapshotResponse, ReadStreamRequest, ReadStreamResponse,
    SnapshotPayloadData, SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotRead, SnapshotType, SnapshotWrite,
    StreamAppend, StreamEvent, StreamPosition, StreamRead, StreamWritePrecondition, WriteSnapshotRequest,
    WriteSnapshotResponse,
};
use trogon_nats::lease::{ReleaseLease, RenewLease, TryAcquireLease};
use trogon_std::{NowV7, UuidV7Generator};

use crate::{
    DeliveryKind, GetScheduleCommand, ListSchedulesCommand, ResolvedSchedule, SamplingSourceKind, ScheduleEventCase,
    ScheduleKind,
    config::{ScheduleWriteCondition, ScheduleWriteState},
    error::SchedulerError,
    projections::{LoadAndWatchSchedulesResult, ScheduleWatchStream},
    read_model::{
        MessageContent, MessageEnvelope, MessageHeaders, Schedule, ScheduleEventDelivery, ScheduleEventSamplingSource,
        ScheduleEventSchedule, ScheduleEventStatus,
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
    type Error = SchedulerError;

    async fn active_schedule_ids(&self) -> Result<HashSet<String>, Self::Error> {
        Ok(self.active.lock().unwrap().clone())
    }

    async fn upsert_schedule(&self, job: &ResolvedSchedule) -> Result<(), Self::Error> {
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
pub struct MockSchedulerStore {
    jobs: Arc<Mutex<HashMap<String, Schedule>>>,
    stream_positions: Arc<Mutex<HashMap<String, StreamPosition>>>,
    events: Arc<Mutex<HashMap<String, Vec<Event>>>>,
    command_snapshots: Arc<Mutex<HashMap<String, HashMap<String, EncodedSnapshot>>>>,
}

#[derive(Clone)]
struct EncodedSnapshot {
    position: StreamPosition,
    payload: Vec<u8>,
}

fn stream_position(value: u64) -> Result<StreamPosition, SchedulerError> {
    StreamPosition::try_new(value)
        .map_err(|source| SchedulerError::event_source("mock stream position must be non-zero", source))
}

fn encode_event<E>(event: &E) -> Event
where
    E: EventType + EventIdentity + EventEncode,
    <E as EventType>::Error: std::fmt::Debug,
    <E as EventEncode>::Error: std::fmt::Debug,
{
    let id = event
        .event_id()
        .unwrap_or_else(|| EventId::new(UuidV7Generator.now_v7()));
    Event {
        id,
        r#type: event.event_type().unwrap().to_string(),
        content: event.encode().unwrap(),
        headers: Headers::empty(),
    }
}

impl MockSchedulerStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seed_job(&self, job: Schedule) {
        let id = job.id.clone();
        let event = v1::ScheduleEvent {
            event: Some(schedule_to_proto_added(&job).into()),
        };

        let initial_position = StreamPosition::new(NonZeroU64::MIN);
        self.stream_positions
            .lock()
            .unwrap()
            .insert(id.clone(), initial_position);
        self.events
            .lock()
            .unwrap()
            .insert(id.clone(), vec![encode_event(&event)]);
        self.jobs.lock().unwrap().insert(id.clone(), job);
    }

    pub(crate) fn read_command_snapshot<Payload>(
        &self,
        stream_id: &(impl AsRef<str> + ?Sized),
    ) -> Result<Option<Snapshot<Payload>>, SchedulerError>
    where
        Payload: SnapshotPayloadDecode + SnapshotType,
        Payload::Error: std::error::Error + Send + Sync + 'static,
    {
        self.command_snapshots
            .lock()
            .unwrap()
            .get(Payload::SNAPSHOT_STREAM_PREFIX)
            .and_then(|snapshots| snapshots.get(stream_id.as_ref()).cloned())
            .map(|snapshot| {
                Payload::decode(SnapshotPayloadData::new(snapshot.payload.as_slice()))
                    .map(|payload| Snapshot::new(snapshot.position, payload))
                    .map_err(|source| SchedulerError::event_source("failed to decode command snapshot payload", source))
            })
            .transpose()
    }

    pub async fn get_schedule(&self, command: GetScheduleCommand) -> Result<Option<Schedule>, SchedulerError> {
        Ok(self.jobs.lock().unwrap().get(command.id.as_str()).cloned())
    }

    pub async fn list_schedules(&self, _command: ListSchedulesCommand) -> Result<Vec<Schedule>, SchedulerError> {
        Ok(self.jobs.lock().unwrap().values().cloned().collect())
    }

    pub async fn load_and_watch_schedules(&self) -> LoadAndWatchSchedulesResult {
        let jobs = self.jobs.lock().unwrap().values().cloned().collect();
        Ok((jobs, Box::pin(futures::stream::pending()) as ScheduleWatchStream))
    }
}

fn schedule_to_proto_added(job: &Schedule) -> v1::ScheduleAdded {
    v1::ScheduleAdded {
        schedule_id: job.id.clone(),
        added_at: crate::commands::domain::proto_timestamp_rfc3339("2026-05-22T00:00:00+00:00").unwrap(),
        status: match job.status {
            ScheduleEventStatus::Enabled => v1::ScheduleStatus::SCHEDULE_STATUS_ENABLED,
            ScheduleEventStatus::Disabled => v1::ScheduleStatus::SCHEDULE_STATUS_DISABLED,
        },
        schedule: MessageField::some(proto_schedule(&job.schedule)),
        delivery: MessageField::some(proto_delivery(&job.delivery)),
        message: MessageField::some(proto_message(&job.message)),
        added_by: buffa::MessageField::some(trogonai_proto::actor::v1alpha1::ActorId {
            value: "test-actor".to_string(),
        }),
    }
}

fn proto_schedule(schedule: &ScheduleEventSchedule) -> v1::Schedule {
    let kind = match schedule {
        ScheduleEventSchedule::At { at } => v1::AtSchedule { at: at.clone() }.into(),
        ScheduleEventSchedule::Every { every_sec } => v1::EverySchedule { every_sec: *every_sec }.into(),
        ScheduleEventSchedule::Cron { expr, timezone } => v1::CronSchedule {
            expr: expr.clone(),
            timezone: timezone.clone().unwrap_or_default(),
        }
        .into(),
        ScheduleEventSchedule::RRule {
            dtstart,
            rrule,
            timezone,
            rdate,
            exdate,
        } => v1::RRuleSchedule {
            dtstart: dtstart.clone(),
            rrule: rrule.clone(),
            timezone: timezone.clone().unwrap_or_default(),
            rdate: rdate.clone(),
            exdate: exdate.clone(),
        }
        .into(),
    };
    v1::Schedule { kind: Some(kind) }
}

fn proto_delivery(delivery: &ScheduleEventDelivery) -> v1::Delivery {
    match delivery {
        ScheduleEventDelivery::NatsEvent { route, ttl_sec, source } => v1::Delivery {
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

fn proto_sampling_source(source: &ScheduleEventSamplingSource) -> v1::SamplingSource {
    match source {
        ScheduleEventSamplingSource::LatestFromSubject { subject } => v1::SamplingSource {
            kind: Some(
                v1::LatestFromSubjectSampling {
                    subject: subject.clone(),
                }
                .into(),
            ),
        },
    }
}

fn proto_message(message: &MessageEnvelope) -> v1::Message {
    v1::Message {
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

fn schedule_read_model_from_proto(stream_id: &str, details: &v1::ScheduleAdded) -> Schedule {
    Schedule {
        id: stream_id.to_string(),
        status: if details.status == v1::ScheduleStatus::SCHEDULE_STATUS_DISABLED {
            ScheduleEventStatus::Disabled
        } else {
            ScheduleEventStatus::Enabled
        },
        schedule: details
            .schedule
            .as_option()
            .map(schedule_from_proto)
            .unwrap_or(ScheduleEventSchedule::Every { every_sec: 0 }),
        delivery: details
            .delivery
            .as_option()
            .map(delivery_from_proto)
            .unwrap_or_else(|| ScheduleEventDelivery::NatsEvent {
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

fn schedule_from_proto(schedule: &v1::Schedule) -> ScheduleEventSchedule {
    match schedule.kind.as_ref() {
        Some(ScheduleKind::At(inner)) => ScheduleEventSchedule::At { at: inner.at.clone() },
        Some(ScheduleKind::Every(inner)) => ScheduleEventSchedule::Every {
            every_sec: inner.every_sec,
        },
        Some(ScheduleKind::Cron(inner)) => ScheduleEventSchedule::Cron {
            expr: inner.expr.clone(),
            timezone: (!inner.timezone.is_empty()).then(|| inner.timezone.clone()),
        },
        Some(ScheduleKind::Rrule(inner)) => ScheduleEventSchedule::RRule {
            dtstart: inner.dtstart.clone(),
            rrule: inner.rrule.clone(),
            timezone: (!inner.timezone.is_empty()).then(|| inner.timezone.clone()),
            rdate: inner.rdate.clone(),
            exdate: inner.exdate.clone(),
        },
        None => ScheduleEventSchedule::Every { every_sec: 0 },
    }
}

fn delivery_from_proto(delivery: &v1::Delivery) -> ScheduleEventDelivery {
    match delivery.kind.as_ref() {
        Some(DeliveryKind::NatsEvent(inner)) => ScheduleEventDelivery::NatsEvent {
            route: inner.route.clone(),
            ttl_sec: inner.ttl_sec,
            source: inner.source.as_option().map(sampling_source_from_proto),
        },
        None => ScheduleEventDelivery::NatsEvent {
            route: String::new(),
            ttl_sec: None,
            source: None,
        },
    }
}

fn sampling_source_from_proto(source: &v1::SamplingSource) -> ScheduleEventSamplingSource {
    match source.kind.as_ref() {
        Some(SamplingSourceKind::LatestFromSubject(inner)) => ScheduleEventSamplingSource::LatestFromSubject {
            subject: inner.subject.clone(),
        },
        None => ScheduleEventSamplingSource::LatestFromSubject { subject: String::new() },
    }
}

fn message_from_proto(message: &v1::Message) -> MessageEnvelope {
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

impl StreamRead<str> for MockSchedulerStore {
    type Error = SchedulerError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        let stream_id = request.stream_id;
        let from_sequence = match request.from {
            ReadFrom::Beginning => 1,
            ReadFrom::Position(position) => position.as_u64(),
        };
        let current_position = self.stream_positions.lock().unwrap().get(stream_id).copied();
        let stream_events = self.events.lock().unwrap().get(stream_id).cloned().unwrap_or_default();

        let mut recorded = Vec::new();
        for (index, event) in stream_events.into_iter().enumerate() {
            let sequence = index as u64 + 1;
            if sequence < from_sequence {
                continue;
            }
            recorded.push(StreamEvent {
                stream_id: stream_id.to_string(),
                event,
                stream_position: stream_position(sequence)?,
                recorded_at: DateTime::<Utc>::from_timestamp(1_700_000_000 + sequence as i64, 0).ok_or_else(|| {
                    SchedulerError::event_source(
                        "failed to build mocked recorded event timestamp",
                        std::io::Error::other(stream_id.to_string()),
                    )
                })?,
            });
        }
        Ok(ReadStreamResponse {
            current_position,
            events: recorded,
        })
    }
}

impl StreamAppend<str> for MockSchedulerStore {
    type Error = SchedulerError;

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
        let write_state = ScheduleWriteState::new(current_position, current_position.is_some());
        match expected_state {
            StreamWritePrecondition::Any => {}
            StreamWritePrecondition::StreamExists if write_state.exists() => {}
            StreamWritePrecondition::StreamExists => {
                return Err(SchedulerError::OptimisticConcurrencyConflict {
                    id: stream_id.to_string(),
                    expected: StreamWritePrecondition::StreamExists,
                    current_position,
                });
            }
            StreamWritePrecondition::NoStream => {
                ScheduleWriteCondition::MustNotExist.ensure(stream_id.as_str(), write_state)?;
            }
            StreamWritePrecondition::At(position) => {
                ScheduleWriteCondition::MustBeAtPosition(position).ensure(stream_id.as_str(), write_state)?;
            }
        }

        let stored_events = stream_events.entry(stream_id.to_string()).or_default();
        let mut projected_job = current_job;
        let mut raw_position = current_position.map(StreamPosition::as_u64).unwrap_or(0);

        for event_data in events {
            let event = v1::ScheduleEvent::decode(EventData::new(&event_data.r#type, &event_data.content))
                .map_err(|source| SchedulerError::event_source("failed to decode mocked job event payload", source))?;
            raw_position += 1;
            stored_events.push(event_data);
            let Some(event) = event.into_decoded() else {
                continue;
            };
            match &event.event {
                Some(ScheduleEventCase::ScheduleAdded(inner)) => {
                    projected_job = Some(schedule_read_model_from_proto(stream_id.as_str(), inner));
                }
                Some(ScheduleEventCase::SchedulePaused(_)) => {
                    let mut job = projected_job.take().ok_or_else(|| {
                        SchedulerError::event_source(
                            "failed to project mocked job pause without current read model",
                            std::io::Error::other(stream_id.to_string()),
                        )
                    })?;
                    job.status = crate::ScheduleEventStatus::Disabled;
                    projected_job = Some(job);
                }
                Some(ScheduleEventCase::ScheduleResumed(_)) => {
                    let mut job = projected_job.take().ok_or_else(|| {
                        SchedulerError::event_source(
                            "failed to project mocked job resume without current read model",
                            std::io::Error::other(stream_id.to_string()),
                        )
                    })?;
                    job.status = crate::ScheduleEventStatus::Enabled;
                    projected_job = Some(job);
                }
                Some(ScheduleEventCase::ScheduleRemoved(_)) => {
                    projected_job = None;
                }
                None => {
                    return Err(SchedulerError::event_source(
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

impl<Payload> SnapshotRead<Payload, str> for MockSchedulerStore
where
    Payload: SnapshotPayloadDecode + SnapshotType + Send,
    Payload::Error: std::error::Error + Send + Sync + 'static,
{
    type Error = SchedulerError;

    async fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, str>,
    ) -> Result<ReadSnapshotResponse<Payload>, Self::Error> {
        self.read_command_snapshot(request.stream_id)
            .map(|snapshot| ReadSnapshotResponse { snapshot })
    }
}

impl<Payload> SnapshotWrite<Payload, str> for MockSchedulerStore
where
    Payload: SnapshotPayloadEncode + SnapshotType + Send,
    Payload::Error: std::error::Error + Send + Sync + 'static,
{
    type Error = SchedulerError;

    async fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, Payload, str>,
    ) -> Result<WriteSnapshotResponse, Self::Error> {
        let snapshot =
            EncodedSnapshot {
                position: request.snapshot.position,
                payload: request.snapshot.payload.encode().map_err(|source| {
                    SchedulerError::event_source("failed to encode command snapshot payload", source)
                })?,
            };
        self.command_snapshots
            .lock()
            .unwrap()
            .entry(Payload::SNAPSHOT_STREAM_PREFIX.to_string())
            .or_default()
            .insert(request.stream_id.to_string(), snapshot);
        Ok(WriteSnapshotResponse)
    }
}

#[cfg(test)]
mod tests {
    use trogon_decider_runtime::{CommandError, CommandExecution, ImmediateSnapshotTaskScheduler};

    use super::*;
    use crate::commands::domain as command_domain;
    use crate::{
        AddScheduleCommand, GetScheduleCommand, ListSchedulesCommand, MessageContent, MessageEnvelope, MessageHeaders,
        PauseScheduleCommand, RemoveScheduleCommand, ResumeScheduleCommand, Schedule, ScheduleActor,
        ScheduleEventDelivery, ScheduleEventSchedule, ScheduleEventStatus, ScheduleId, ScheduleWriteCondition,
    };

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).expect("test stream position must be non-zero")
    }
    use futures::StreamExt;

    fn command_job_id(id: &str) -> command_domain::ScheduleId {
        command_domain::ScheduleId::parse(id).unwrap()
    }

    fn actor() -> ScheduleActor {
        ScheduleActor::parse("test-actor").unwrap()
    }

    fn event_at() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-05-22T00:00:00+00:00")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    fn base_job(id: &str) -> Schedule {
        Schedule {
            id: id.to_string(),
            status: ScheduleEventStatus::Enabled,
            schedule: ScheduleEventSchedule::Every { every_sec: 30 },
            delivery: ScheduleEventDelivery::NatsEvent {
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

    fn expected_job(id: &str) -> Schedule {
        base_job(id)
    }

    #[tokio::test]
    async fn mock_schedule_publisher_tracks_active_jobs() {
        let publisher = MockSchedulePublisher::new();
        publisher.seed_active_job("orphan");
        let details = schedule_to_proto_added(&expected_job("alpha"));
        let resolved = ResolvedSchedule::from_event("alpha", &details).unwrap();

        let active = publisher.active_schedule_ids().await.unwrap();
        assert!(active.contains("orphan"));

        publisher.upsert_schedule(&resolved).await.unwrap();
        publisher.remove_schedule("orphan").await.unwrap();

        assert_eq!(publisher.upserts(), vec!["scheduler.schedules.alpha"]);
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
    async fn mock_scheduler_store_covers_crud_and_read_model_watch() {
        let store = MockSchedulerStore::new();
        store.seed_job(base_job("seeded"));

        let seeded = store
            .get_schedule(GetScheduleCommand::new(ScheduleId::parse("seeded").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(seeded, expected_job("seeded"));

        CommandExecution::new(
            &store,
            &AddScheduleCommand::new(command_base_job("alpha"), actor(), event_at()),
        )
        .with_snapshot(&store)
        .with_task_runtime(ImmediateSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap();
        let alpha = store
            .get_schedule(GetScheduleCommand::new(ScheduleId::parse("alpha").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(alpha, expected_job("alpha"));

        CommandExecution::new(
            &store,
            &PauseScheduleCommand::new(command_job_id("alpha"), actor(), event_at()),
        )
        .with_snapshot(&store)
        .with_task_runtime(ImmediateSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap();
        assert_eq!(
            store
                .get_schedule(GetScheduleCommand::new(ScheduleId::parse("alpha").unwrap()))
                .await
                .unwrap()
                .unwrap()
                .status,
            ScheduleEventStatus::Disabled
        );

        let listed = store.list_schedules(ListSchedulesCommand).await.unwrap();
        assert_eq!(listed.len(), 2);

        let (watch_jobs, mut watcher) = store.load_and_watch_schedules().await.unwrap();
        assert_eq!(watch_jobs.len(), 2);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(5), watcher.next())
                .await
                .is_err()
        );

        CommandExecution::new(
            &store,
            &RemoveScheduleCommand::new(command_job_id("alpha"), actor(), event_at()),
        )
        .with_snapshot(&store)
        .with_task_runtime(ImmediateSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap();
        assert!(
            store
                .get_schedule(GetScheduleCommand::new(ScheduleId::parse("alpha").unwrap()))
                .await
                .unwrap()
                .is_none()
        );

        let deleted_error = CommandExecution::new(
            &store,
            &AddScheduleCommand::new(command_base_job("alpha"), actor(), event_at()),
        )
        .with_snapshot(&store)
        .with_task_runtime(ImmediateSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap_err();
        assert!(matches!(
            deleted_error,
            CommandError::Decide(crate::AddScheduleDecideError::JobDeleted { .. })
        ));
    }

    #[tokio::test]
    async fn mock_scheduler_store_rejects_invalid_specs_and_state_errors() {
        let store = MockSchedulerStore::new();
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

        CommandExecution::new(
            &store,
            &AddScheduleCommand::new(command_base_job("alpha"), actor(), event_at()),
        )
        .with_snapshot(&store)
        .with_task_runtime(ImmediateSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap();
        let same_state_error = CommandExecution::new(
            &store,
            &ResumeScheduleCommand::new(command_job_id("alpha"), actor(), event_at()),
        )
        .with_snapshot(&store)
        .with_task_runtime(ImmediateSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap_err();
        assert!(matches!(
            same_state_error,
            CommandError::Decide(crate::ResumeScheduleDecideError::AlreadyActive { .. })
        ));

        let missing_error = CommandExecution::new(
            &store,
            &PauseScheduleCommand::new(command_job_id("missing"), actor(), event_at()),
        )
        .with_snapshot(&store)
        .with_task_runtime(ImmediateSnapshotTaskScheduler)
        .execute()
        .await
        .unwrap_err();
        assert!(matches!(
            missing_error,
            CommandError::Decide(crate::PauseScheduleDecideError::JobNotFound { .. })
        ));
    }

    #[test]
    fn ensure_write_condition_covers_accept_and_conflict_paths() {
        ScheduleWriteCondition::MustNotExist
            .ensure("alpha", ScheduleWriteState::new(None, false))
            .unwrap();
        ScheduleWriteCondition::MustBeAtPosition(position(3))
            .ensure("alpha", ScheduleWriteState::new(Some(position(3)), true))
            .unwrap();

        let error = ScheduleWriteCondition::MustNotExist
            .ensure("alpha", ScheduleWriteState::new(Some(position(4)), true))
            .unwrap_err();
        assert!(matches!(
            error,
            SchedulerError::OptimisticConcurrencyConflict {
                current_position: Some(_),
                ..
            }
        ));

        let error = ScheduleWriteCondition::MustBeAtPosition(position(3))
            .ensure("alpha", ScheduleWriteState::new(Some(position(4)), true))
            .unwrap_err();
        assert!(matches!(
            error,
            SchedulerError::OptimisticConcurrencyConflict {
                current_position: Some(_),
                ..
            }
        ));
    }
}
