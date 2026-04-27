use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

use async_nats::jetstream::{
    self,
    consumer::{AckPolicy, DeliverPolicy, ReplayPolicy, pull},
    context::ConsumerInfoErrorKind,
};
use futures::{Stream, StreamExt};
use trogon_eventsourcing::record_stream_message;
use trogon_nats::SubjectTokenViolation;
use trogon_nats::lease::{LeaderElection, LeaseRenewInterval, LeaseTiming, LeaseTtl, NatsKvLease, NatsKvLeaseConfig};
use trogon_std::{NowV7, UuidV7Generator};

use crate::{
    CronJob, JobEventProtoError, JobEventStatus, RecordedJobEvent, ResolvedJob,
    commands::proto::{JobContractEventCodec, contract_event_stream_id, contract_v1},
    error::CronError,
    kv::{EVENTS_SUBJECT_PREFIX, LEADER_BUCKET, LEADER_KEY, LEGACY_EVENTS_SUBJECT_PREFIX},
    nats::NatsSchedulePublisher,
    store::{Store, connect_store},
    traits::{LeaderLock, SchedulePublisher},
};

const WATCH_RETRY_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_LEADER_RENEW_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_LEADER_TTL: Duration = Duration::from_secs(10);
const SCHEDULER_CONSUMER_NAME: &str = "cron_scheduler";
const SCHEDULER_CONSUMER_INACTIVE_THRESHOLD: Duration = Duration::from_secs(30);

type DesiredJobs = HashMap<String, DesiredJobState>;
type SchedulerEventWatcher = Pin<Box<dyn Stream<Item = Result<jetstream::Message, CronError>> + Send + 'static>>;

enum ReestablishedProcessor {
    Ready((DesiredJobs, SchedulerEventWatcher)),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq)]
enum SchedulerChange {
    Upsert(CronJob),
    Delete(String),
}

#[derive(Debug, Clone, PartialEq)]
enum DesiredJobState {
    Present(Box<CronJob>),
    Deleted,
}

pub struct CronController<C = Store, P = NatsSchedulePublisher, L = NatsKvLease> {
    store: C,
    schedule_publisher: P,
    leader_lock: L,
    node_id: String,
    leader_timing: LeaseTiming,
}

impl CronController<Store, NatsSchedulePublisher, NatsKvLease> {
    pub async fn from_nats(nats: async_nats::Client) -> Result<Self, CronError> {
        let js = async_nats::jetstream::new(nats.clone());
        let store = connect_store(nats.clone()).await?;
        let schedule_publisher = NatsSchedulePublisher::new(nats).await?;
        let leader_timing = default_leader_timing()?;
        let leader_config = NatsKvLeaseConfig::new(
            LEADER_BUCKET,
            LEADER_KEY,
            LeaseTtl::from_secs(DEFAULT_LEADER_TTL.as_secs())
                .map_err(|source| CronError::lease_source("invalid default leader TTL", source))?,
            LeaseRenewInterval::from_secs(DEFAULT_LEADER_RENEW_INTERVAL.as_secs())
                .map_err(|source| CronError::lease_source("invalid default leader renew interval", source))?,
        )
        .map_err(|source| CronError::lease_source("invalid leader lease config", source))?;
        let leader_lock = NatsKvLease::provision(&js, &leader_config)
            .await
            .map_err(|source| CronError::lease_source("failed to provision leader lease", source))?;

        Ok(Self {
            store,
            schedule_publisher,
            leader_lock,
            node_id: UuidV7Generator.now_v7().to_string(),
            leader_timing,
        })
    }
}

impl<C, P, L> CronController<C, P, L> {
    pub fn new(store: C, schedule_publisher: P, leader_lock: L) -> Result<Self, CronError> {
        Ok(Self {
            store,
            schedule_publisher,
            leader_lock,
            node_id: UuidV7Generator.now_v7().to_string(),
            leader_timing: default_leader_timing()?,
        })
    }

    pub fn with_node_id(mut self, node_id: String) -> Self {
        self.node_id = node_id;
        self
    }
}

impl<P, L> CronController<Store, P, L>
where
    P: SchedulePublisher<Error = CronError>,
    L: LeaderLock,
{
    pub async fn run(self) -> Result<(), CronError> {
        let mut desired_jobs = DesiredJobs::new();
        let mut scheduler_watcher = inactive_scheduler_watcher();
        let mut leader = LeaderElection::new(self.leader_lock, self.node_id.clone(), self.leader_timing);
        let mut currently_leader = false;
        let mut heartbeat = tokio::time::interval(self.leader_timing.renew_interval() / 2);

        loop {
            tokio::select! {
                _ = trogon_telemetry::signal::shutdown_signal() => {
                    tracing::info!("Shutdown signal received, releasing leader lease");
                    if let Err(error) = leader.release().await {
                        tracing::warn!(error = %error, "Failed to release leader lease");
                    }
                    break;
                }
                _ = heartbeat.tick() => {
                    match leader.ensure_leader().await {
                        Ok(is_leader) => {
                            if is_leader && !currently_leader {
                                tracing::info!(node_id = %self.node_id, "Controller became leader");
                                let (rebuilt_jobs, watcher) = establish_scheduler_processor(
                                    &self.store,
                                    &self.schedule_publisher,
                                ).await?;
                                desired_jobs = rebuilt_jobs;
                                scheduler_watcher = watcher;
                            } else if !is_leader && currently_leader {
                                tracing::info!(node_id = %self.node_id, "Controller lost leadership");
                                desired_jobs.clear();
                                scheduler_watcher = inactive_scheduler_watcher();
                            }
                            currently_leader = is_leader;
                        }
                        Err(source) => {
                            tracing::warn!(
                                error = %source,
                                node_id = %self.node_id,
                                was_leader = currently_leader,
                                "Controller leadership check failed, stepping down and retrying"
                            );
                            currently_leader = false;
                            desired_jobs.clear();
                            scheduler_watcher = inactive_scheduler_watcher();
                        }
                    }
                }
                message = scheduler_watcher.next() => {
                    match message {
                        Some(Ok(message)) => {
                            let change = match handle_scheduler_message(&mut desired_jobs, &message).await {
                                Ok(change) => change,
                                Err(error) => {
                                    tracing::error!(error = %error, "Failed to apply scheduler event");
                                    ack_scheduler_message(&message).await;
                                    continue;
                                }
                            };
                            apply_scheduler_change(&self.schedule_publisher, &change).await?;
                            ack_scheduler_message(&message).await;
                        }
                        Some(Err(error)) => {
                            tracing::warn!(
                                error = %error,
                                retry_ms = WATCH_RETRY_INTERVAL.as_millis(),
                                "Scheduler event watcher returned an error, attempting to re-establish it"
                            );
                            match reestablish_scheduler_processor(&self.store, &self.schedule_publisher).await? {
                                ReestablishedProcessor::Ready((jobs, watcher)) => {
                                    desired_jobs = jobs;
                                    scheduler_watcher = watcher;
                                }
                                ReestablishedProcessor::Shutdown => {
                                    tracing::info!("Shutdown received while re-establishing scheduler processor, releasing leader lease");
                                    if let Err(error) = leader.release().await {
                                        tracing::warn!(error = %error, "Failed to release leader lease");
                                    }
                                    break;
                                }
                            }
                        }
                        None => {
                            tracing::warn!(
                                retry_ms = WATCH_RETRY_INTERVAL.as_millis(),
                                "Scheduler event watcher ended, attempting to re-establish it"
                            );
                            match reestablish_scheduler_processor(&self.store, &self.schedule_publisher).await? {
                                ReestablishedProcessor::Ready((jobs, watcher)) => {
                                    desired_jobs = jobs;
                                    scheduler_watcher = watcher;
                                }
                                ReestablishedProcessor::Shutdown => {
                                    tracing::info!("Shutdown received while re-establishing scheduler processor, releasing leader lease");
                                    if let Err(error) = leader.release().await {
                                        tracing::warn!(error = %error, "Failed to release leader lease");
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

fn default_leader_timing() -> Result<LeaseTiming, CronError> {
    let ttl = LeaseTtl::from_secs(DEFAULT_LEADER_TTL.as_secs())
        .map_err(|source| CronError::lease_source("invalid default leader TTL", source))?;
    let renew_interval = LeaseRenewInterval::from_secs(DEFAULT_LEADER_RENEW_INTERVAL.as_secs())
        .map_err(|source| CronError::lease_source("invalid default leader renew interval", source))?;

    LeaseTiming::new(ttl, renew_interval)
        .map_err(|source| CronError::lease_source("invalid default leader timing", source))
}

async fn establish_scheduler_processor<P>(
    store: &Store,
    publisher: &P,
) -> Result<(DesiredJobs, SchedulerEventWatcher), CronError>
where
    P: SchedulePublisher<Error = CronError>,
{
    let stream = store.event_store.events_stream().clone();
    let info = stream
        .get_info()
        .await
        .map_err(|source| CronError::event_source("failed to query events stream info", source))?;
    let desired_jobs =
        rebuild_scheduler_state_from_stream(&stream, info.state.first_sequence, info.state.last_sequence).await?;
    reconcile_snapshot(publisher, &desired_jobs).await?;
    let watcher =
        open_scheduler_event_watcher(&stream, next_scheduler_start_sequence(info.state.last_sequence)).await?;
    Ok((desired_jobs, watcher))
}

async fn reestablish_scheduler_processor<P>(store: &Store, publisher: &P) -> Result<ReestablishedProcessor, CronError>
where
    P: SchedulePublisher<Error = CronError>,
{
    loop {
        tokio::select! {
            _ = trogon_telemetry::signal::shutdown_signal() => {
                return Ok(ReestablishedProcessor::Shutdown);
            }
            result = establish_scheduler_processor(store, publisher) => {
                match result {
                    Ok((jobs, watcher)) => {
                        return Ok(ReestablishedProcessor::Ready((jobs, watcher)));
                    }
                    Err(error) => {
                        tracing::error!(
                            error = %error,
                            retry_ms = WATCH_RETRY_INTERVAL.as_millis(),
                            "Failed to re-establish scheduler processor"
                        );
                    }
                }
            }
        }

        tokio::time::sleep(WATCH_RETRY_INTERVAL).await;
    }
}

async fn rebuild_scheduler_state_from_stream(
    stream: &jetstream::stream::Stream,
    first_sequence: u64,
    last_sequence: u64,
) -> Result<DesiredJobs, CronError> {
    let mut desired_jobs = DesiredJobs::new();
    if last_sequence == 0 || first_sequence == 0 || first_sequence > last_sequence {
        return Ok(desired_jobs);
    }

    for sequence in first_sequence..=last_sequence {
        let Some(message) =
            read_raw_scheduler_event_message(stream, sequence, "failed to read job event from stream").await?
        else {
            continue;
        };
        let event = decode_recorded_job_event(message)?;
        let stream_id = job_id_from_event_subject(&event.recorded_stream_id)?;
        let data = event
            .decode_data_with(&JobContractEventCodec)
            .map_err(|source| CronError::event_source("failed to decode recorded job event payload", source))?;
        ensure_event_matches_stream(&stream_id, &data)?;
        let _ = apply_scheduler_event(&mut desired_jobs, &data)?;
    }

    Ok(desired_jobs)
}

fn apply_scheduler_event(
    desired_jobs: &mut DesiredJobs,
    event: &contract_v1::JobEvent,
) -> Result<SchedulerChange, CronError> {
    match event.event() {
        contract_v1::job_event::EventOneof::JobAdded(inner) => {
            let id = inner.id().to_string();
            if matches!(desired_jobs.get(&id), Some(DesiredJobState::Deleted)) {
                return Err(CronError::event_source(
                    "scheduler received an add event for a deleted job stream",
                    std::io::Error::other(id),
                ));
            }
            validate_event_job_id(&id).map_err(|source| {
                CronError::invalid_job_spec(crate::JobSpecError::InvalidId { id: id.clone(), source })
            })?;
            if !inner.has_job() {
                return Err(CronError::event_source(
                    "scheduler received a job_added event without job details",
                    JobEventProtoError::MissingJobDetails,
                ));
            }
            let job = CronJob::try_from((id.clone(), inner.job().to_owned()))
                .map_err(|source| CronError::event_source("failed to decode scheduler job details", source))?;
            desired_jobs.insert(job.id.clone(), DesiredJobState::Present(Box::new(job.clone())));
            Ok(SchedulerChange::Upsert(job))
        }
        contract_v1::job_event::EventOneof::JobPaused(inner) => {
            let id = inner.id().to_string();
            let job = desired_jobs.get_mut(&id).ok_or_else(|| {
                CronError::event_source(
                    "scheduler received a pause without current job state",
                    std::io::Error::other(id.clone()),
                )
            })?;
            match job {
                DesiredJobState::Present(job) => {
                    job.status = JobEventStatus::Disabled;
                    Ok(SchedulerChange::Delete(id))
                }
                DesiredJobState::Deleted => Err(CronError::event_source(
                    "scheduler received a pause for a deleted job stream",
                    std::io::Error::other(id),
                )),
            }
        }
        contract_v1::job_event::EventOneof::JobResumed(inner) => {
            let id = inner.id().to_string();
            let job = desired_jobs.get_mut(&id).ok_or_else(|| {
                CronError::event_source(
                    "scheduler received a resume without current job state",
                    std::io::Error::other(id.clone()),
                )
            })?;
            match job {
                DesiredJobState::Present(job) => {
                    job.status = JobEventStatus::Enabled;
                    Ok(SchedulerChange::Upsert(job.as_ref().clone()))
                }
                DesiredJobState::Deleted => Err(CronError::event_source(
                    "scheduler received a resume for a deleted job stream",
                    std::io::Error::other(id),
                )),
            }
        }
        contract_v1::job_event::EventOneof::JobRemoved(inner) => {
            let id = inner.id().to_string();
            desired_jobs.insert(id.clone(), DesiredJobState::Deleted);
            Ok(SchedulerChange::Delete(id))
        }
        contract_v1::job_event::EventOneof::not_set(_) | _ => Err(CronError::event_source(
            "scheduler received an event without a oneof case",
            JobEventProtoError::MissingEvent,
        )),
    }
}

async fn handle_scheduler_message(
    desired_jobs: &mut DesiredJobs,
    message: &jetstream::Message,
) -> Result<SchedulerChange, CronError> {
    let event = decode_recorded_watch_message(message)?;
    let stream_id = job_id_from_event_subject(&event.recorded_stream_id)?;
    let data = event
        .decode_data_with(&JobContractEventCodec)
        .map_err(|source| CronError::event_source("failed to decode watched scheduler event payload", source))?;
    ensure_event_matches_stream(&stream_id, &data)?;
    apply_scheduler_event(desired_jobs, &data)
}

async fn apply_scheduler_change<P: SchedulePublisher<Error = CronError>>(
    publisher: &P,
    change: &SchedulerChange,
) -> Result<(), CronError> {
    match change {
        SchedulerChange::Upsert(job) => {
            if job.is_enabled() {
                match ResolvedJob::try_from(job) {
                    Ok(resolved) => {
                        publisher.upsert_schedule(&resolved).await?;
                    }
                    Err(error) => {
                        tracing::error!(
                            error = %error,
                            job_id = %job.id,
                            "Skipping invalid enabled scheduler job and removing any existing schedule"
                        );
                        publisher.remove_schedule(&job.id).await?;
                    }
                }
            } else {
                publisher.remove_schedule(&job.id).await?;
            }
        }
        SchedulerChange::Delete(id) => {
            publisher.remove_schedule(id).await?;
        }
    }

    Ok(())
}

async fn reconcile_snapshot<P: SchedulePublisher<Error = CronError>>(
    publisher: &P,
    desired_jobs: &DesiredJobs,
) -> Result<(), CronError> {
    let mut desired_active_ids = std::collections::HashSet::new();
    let mut resolved_jobs = Vec::new();

    for job in desired_jobs.values() {
        let DesiredJobState::Present(job) = job else {
            continue;
        };
        if !job.is_enabled() {
            continue;
        }

        match ResolvedJob::try_from(job.as_ref()) {
            Ok(resolved) => {
                desired_active_ids.insert(job.id.to_string());
                resolved_jobs.push(resolved);
            }
            Err(error) => {
                tracing::error!(
                    error = %error,
                    job_id = %job.id,
                    "Skipping invalid enabled job during reconciliation"
                );
            }
        }
    }

    let active_schedule_ids = publisher.active_schedule_ids().await?;
    let stale_ids = active_schedule_ids
        .difference(&desired_active_ids)
        .cloned()
        .collect::<Vec<_>>();

    for id in stale_ids {
        publisher.remove_schedule(&id).await?;
    }

    for resolved in resolved_jobs {
        publisher.upsert_schedule(&resolved).await?;
    }

    Ok(())
}

fn inactive_scheduler_watcher() -> SchedulerEventWatcher {
    Box::pin(futures::stream::pending::<Result<jetstream::Message, CronError>>())
}

async fn open_scheduler_event_watcher(
    stream: &jetstream::stream::Stream,
    start_sequence: u64,
) -> Result<SchedulerEventWatcher, CronError> {
    recreate_scheduler_consumer(stream, start_sequence).await?;
    let consumer = stream
        .get_consumer::<pull::Config>(SCHEDULER_CONSUMER_NAME)
        .await
        .map_err(|source| {
            CronError::event_source(
                "failed to open scheduler event consumer",
                std::io::Error::other(source.to_string()),
            )
        })?;
    let messages = consumer
        .messages()
        .await
        .map_err(|source| CronError::event_source("failed to open scheduler event watch stream", source))?;

    Ok(Box::pin(messages.map(|result| {
        result.map_err(|source| CronError::event_source("failed to read scheduler event from consumer", source))
    })))
}

async fn recreate_scheduler_consumer(stream: &jetstream::stream::Stream, start_sequence: u64) -> Result<(), CronError> {
    match stream.consumer_info(SCHEDULER_CONSUMER_NAME).await {
        Ok(_) => {
            stream
                .delete_consumer(SCHEDULER_CONSUMER_NAME)
                .await
                .map_err(|source| CronError::event_source("failed to delete existing scheduler consumer", source))?;
        }
        Err(error) if matches!(error.kind(), ConsumerInfoErrorKind::NotFound) => {}
        Err(error) => {
            return Err(CronError::event_source(
                "failed to query existing scheduler consumer",
                error,
            ));
        }
    }

    stream
        .create_consumer(scheduler_consumer_config(start_sequence))
        .await
        .map_err(|source| CronError::event_source("failed to create scheduler event consumer", source))?;

    Ok(())
}

async fn read_raw_scheduler_event_message(
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

fn decode_recorded_job_event(
    message: async_nats::jetstream::message::StreamMessage,
) -> Result<RecordedJobEvent, CronError> {
    record_stream_message(message)
        .map_err(|source| CronError::event_source("failed to decode stored job event", source))
}

fn decode_recorded_watch_message(message: &async_nats::jetstream::Message) -> Result<RecordedJobEvent, CronError> {
    let stream_message =
        async_nats::jetstream::message::StreamMessage::try_from(message.message.clone()).map_err(|source| {
            CronError::event_source(
                "failed to reconstruct stream message from scheduler watch delivery",
                source,
            )
        })?;

    decode_recorded_job_event(stream_message)
}

fn next_scheduler_start_sequence(last_sequence: u64) -> u64 {
    last_sequence.saturating_add(1).max(1)
}

fn scheduler_consumer_config(start_sequence: u64) -> pull::Config {
    pull::Config {
        durable_name: Some(SCHEDULER_CONSUMER_NAME.to_string()),
        name: Some(SCHEDULER_CONSUMER_NAME.to_string()),
        deliver_policy: DeliverPolicy::ByStartSequence { start_sequence },
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        inactive_threshold: SCHEDULER_CONSUMER_INACTIVE_THRESHOLD,
        ..Default::default()
    }
}

async fn ack_scheduler_message(message: &jetstream::Message) {
    if let Err(error) = message.ack().await {
        tracing::error!(error = %error, "Failed to acknowledge scheduler event");
    }
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

fn ensure_event_matches_stream(expected_stream_id: &str, event: &contract_v1::JobEvent) -> Result<(), CronError> {
    let event_stream_id = contract_event_stream_id(event)
        .map_err(|source| CronError::event_source("scheduler event is missing its id", source))?;
    if event_stream_id == expected_stream_id {
        Ok(())
    } else {
        Err(CronError::event_source(
            "event payload stream id does not match the expected stream",
            std::io::Error::other(format!(
                "expected '{}' but event carried '{}'",
                expected_stream_id, event_stream_id
            )),
        ))
    }
}

fn validate_event_job_id(id: &str) -> Result<(), SubjectTokenViolation> {
    trogon_nats::NatsToken::new(id).map(|_| ())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy};

    use super::{
        CronController, DesiredJobState, SchedulerChange, apply_scheduler_change, apply_scheduler_event,
        default_leader_timing, next_scheduler_start_sequence, reconcile_snapshot, scheduler_consumer_config,
    };
    use crate::commands::proto::contract_v1;
    use crate::{
        CronJob, Delivery, Job, JobDetails, JobEventStatus, JobHeaders, JobId, JobMessage, JobStatus, MessageContent,
        MessageEnvelope, MessageHeaders, Schedule,
        mocks::{MockCronStore, MockLeaderLock, MockSchedulePublisher},
    };

    fn job_id(id: &str) -> JobId {
        JobId::parse(id).unwrap()
    }

    fn base_job(id: &str) -> Job {
        Job {
            id: job_id(id),
            status: JobStatus::Enabled,
            schedule: Schedule::every(30).unwrap(),
            delivery: Delivery::nats_event("agent.run").unwrap(),
            message: JobMessage {
                content: MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
                headers: JobHeaders::default(),
            },
        }
    }

    fn expected_job(id: &str) -> CronJob {
        CronJob::from((id.to_string(), JobDetails::from(base_job(id))))
    }

    fn added_event(id: &str) -> contract_v1::JobEvent {
        let mut event = contract_v1::JobEvent::new();
        let mut inner = contract_v1::JobAdded::new();
        inner.set_id(id);
        inner.set_job(contract_v1::JobDetails::from(&JobDetails::from(base_job(id))));
        event.set_job_added(inner);
        event
    }

    fn paused_event(id: &str) -> contract_v1::JobEvent {
        let mut event = contract_v1::JobEvent::new();
        let mut inner = contract_v1::JobPaused::new();
        inner.set_id(id);
        event.set_job_paused(inner);
        event
    }

    fn resumed_event(id: &str) -> contract_v1::JobEvent {
        let mut event = contract_v1::JobEvent::new();
        let mut inner = contract_v1::JobResumed::new();
        inner.set_id(id);
        event.set_job_resumed(inner);
        event
    }

    fn removed_event(id: &str) -> contract_v1::JobEvent {
        let mut event = contract_v1::JobEvent::new();
        let mut inner = contract_v1::JobRemoved::new();
        inner.set_id(id);
        event.set_job_removed(inner);
        event
    }

    fn invalid_enabled_job(id: &str) -> CronJob {
        CronJob {
            id: id.to_string(),
            status: JobEventStatus::Enabled,
            schedule: crate::JobEventSchedule::Cron {
                expr: "not-a-cron".to_string(),
                timezone: None,
            },
            delivery: crate::JobEventDelivery::NatsEvent {
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

    #[tokio::test]
    async fn reconcile_snapshot_removes_orphaned_schedules_from_previous_leader() {
        let publisher = MockSchedulePublisher::new();
        publisher.seed_active_job("orphan");
        publisher.seed_active_job("heartbeat");

        let desired_jobs = HashMap::from([(
            "heartbeat".to_string(),
            DesiredJobState::Present(Box::new(expected_job("heartbeat"))),
        )]);

        reconcile_snapshot(&publisher, &desired_jobs).await.unwrap();

        assert_eq!(publisher.removals(), vec!["orphan"]);
        assert_eq!(publisher.upserts(), vec!["cron.schedules.heartbeat"]);
    }

    #[tokio::test]
    async fn reconcile_snapshot_removes_disabled_jobs_without_resolution() {
        let publisher = MockSchedulePublisher::new();
        publisher.seed_active_job("disabled");

        let mut disabled = base_job("disabled");
        disabled.status = JobStatus::Disabled;
        let desired_jobs = HashMap::from([(
            "disabled".to_string(),
            DesiredJobState::Present(Box::new(CronJob::from((
                "disabled".to_string(),
                JobDetails::from(disabled),
            )))),
        )]);

        reconcile_snapshot(&publisher, &desired_jobs).await.unwrap();

        assert_eq!(publisher.removals(), vec!["disabled"]);
        assert!(publisher.upserts().is_empty());
    }

    #[test]
    fn controller_construction_and_helpers_set_expected_state() {
        let controller = CronController::new(
            MockCronStore::new(),
            MockSchedulePublisher::new(),
            MockLeaderLock::new(),
        )
        .unwrap()
        .with_node_id("node-1".to_string());
        let default_leader_timing = default_leader_timing().unwrap();

        assert_eq!(controller.node_id, "node-1");
        assert_eq!(controller.leader_timing.ttl(), default_leader_timing.ttl());
        assert_eq!(
            controller.leader_timing.renew_interval(),
            default_leader_timing.renew_interval()
        );
    }

    #[test]
    fn desired_jobs_map_tracks_snapshot_state() {
        let jobs = HashMap::from([(
            "alpha".to_string(),
            DesiredJobState::Present(Box::new(expected_job("alpha"))),
        )]);
        assert_eq!(jobs.keys().cloned().collect::<Vec<_>>(), vec!["alpha"]);
    }

    #[test]
    fn apply_scheduler_event_tracks_register_disable_enable_and_terminal_delete() {
        let mut desired_jobs = HashMap::new();

        let added = apply_scheduler_event(&mut desired_jobs, &added_event("alpha")).unwrap();
        assert_eq!(added, SchedulerChange::Upsert(expected_job("alpha")));
        assert!(matches!(desired_jobs.get("alpha"), Some(DesiredJobState::Present(_))));

        let disabled = apply_scheduler_event(&mut desired_jobs, &paused_event("alpha")).unwrap();
        assert_eq!(disabled, SchedulerChange::Delete("alpha".to_string()));
        assert_eq!(
            match desired_jobs.get("alpha").unwrap() {
                DesiredJobState::Present(job) => job.status,
                DesiredJobState::Deleted => panic!("expected present job"),
            },
            JobEventStatus::Disabled
        );

        let enabled = apply_scheduler_event(&mut desired_jobs, &resumed_event("alpha")).unwrap();
        assert_eq!(enabled, SchedulerChange::Upsert(expected_job("alpha")));

        let removed = apply_scheduler_event(&mut desired_jobs, &removed_event("alpha")).unwrap();
        assert_eq!(removed, SchedulerChange::Delete("alpha".to_string()));
        assert!(matches!(desired_jobs.get("alpha"), Some(DesiredJobState::Deleted)));

        let error = apply_scheduler_event(&mut desired_jobs, &added_event("alpha")).unwrap_err();
        assert!(error.to_string().contains("deleted job stream"));
    }

    #[test]
    fn apply_scheduler_event_rejects_pause_without_current_job() {
        let mut desired_jobs = HashMap::new();
        let error = apply_scheduler_event(&mut desired_jobs, &paused_event("missing")).unwrap_err();

        assert!(error.to_string().contains("pause"));
    }

    #[tokio::test]
    async fn apply_scheduler_change_upserts_enabled_jobs() {
        let publisher = MockSchedulePublisher::new();

        apply_scheduler_change(&publisher, &SchedulerChange::Upsert(expected_job("enabled")))
            .await
            .unwrap();

        assert_eq!(publisher.upserts(), vec!["cron.schedules.enabled"]);
        assert!(publisher.removals().is_empty());
    }

    #[tokio::test]
    async fn apply_scheduler_change_removes_disabled_and_deleted_jobs() {
        let publisher = MockSchedulePublisher::new();
        let mut disabled = base_job("disabled");
        disabled.status = JobStatus::Disabled;

        apply_scheduler_change(
            &publisher,
            &SchedulerChange::Upsert(CronJob::from(("disabled".to_string(), JobDetails::from(disabled)))),
        )
        .await
        .unwrap();
        apply_scheduler_change(&publisher, &SchedulerChange::Delete("deleted".to_string()))
            .await
            .unwrap();

        assert!(publisher.upserts().is_empty());
        assert_eq!(publisher.removals(), vec!["disabled", "deleted"]);
    }

    #[tokio::test]
    async fn apply_scheduler_change_skips_invalid_enabled_jobs_and_removes_existing_schedule() {
        let publisher = MockSchedulePublisher::new();
        publisher.seed_active_job("invalid");

        apply_scheduler_change(&publisher, &SchedulerChange::Upsert(invalid_enabled_job("invalid")))
            .await
            .unwrap();

        assert!(publisher.upserts().is_empty());
        assert_eq!(publisher.removals(), vec!["invalid"]);
    }

    #[tokio::test]
    async fn reconcile_snapshot_skips_invalid_enabled_jobs_without_blocking_valid_ones() {
        let publisher = MockSchedulePublisher::new();
        publisher.seed_active_job("invalid");
        let desired_jobs = HashMap::from([
            (
                "valid".to_string(),
                DesiredJobState::Present(Box::new(expected_job("valid"))),
            ),
            (
                "invalid".to_string(),
                DesiredJobState::Present(Box::new(invalid_enabled_job("invalid"))),
            ),
        ]);

        reconcile_snapshot(&publisher, &desired_jobs).await.unwrap();

        assert_eq!(publisher.removals(), vec!["invalid"]);
        assert_eq!(publisher.upserts(), vec!["cron.schedules.valid"]);
    }

    #[test]
    fn scheduler_consumer_helpers_use_expected_values() {
        let config = scheduler_consumer_config(42);
        assert_eq!(config.durable_name.as_deref(), Some("cron_scheduler"));
        assert_eq!(config.name.as_deref(), Some("cron_scheduler"));
        assert_eq!(next_scheduler_start_sequence(0), 1);
        assert_eq!(next_scheduler_start_sequence(41), 42);
        assert!(matches!(
            config.deliver_policy,
            DeliverPolicy::ByStartSequence { start_sequence: 42 }
        ));
        assert_eq!(config.ack_policy, AckPolicy::Explicit);
        assert_eq!(config.replay_policy, ReplayPolicy::Instant);
    }
}
