use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

use async_nats::jetstream::{
    self,
    consumer::{AckPolicy, DeliverPolicy, ReplayPolicy, pull},
    context::ConsumerInfoErrorKind,
};
use futures::{Stream, StreamExt, future};
use trogon_eventsourcing::{RecordedEvent, record_stream_message};
use trogon_nats::SubjectTokenViolation;
use trogon_nats::lease::{LeaderElection, LeaseRenewInterval, LeaseTiming, LeaseTtl, NatsKvLease, NatsKvLeaseConfig};
use trogon_std::{NowV7, UuidV7Generator, signal};

use crate::{
    ResolvedJob,
    error::CronError,
    kv::{EVENTS_SUBJECT_PREFIX, LEADER_BUCKET, LEADER_KEY},
    nats::NatsSchedulePublisher,
    proto::{JobEventCodec, v1},
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
    Upsert(SchedulerJob),
    Delete(String),
}

#[derive(Debug, Clone, PartialEq)]
enum DesiredJobState {
    Present(Box<SchedulerJob>),
    Deleted,
}

#[derive(Debug, Clone)]
struct SchedulerJob {
    id: String,
    details: v1::JobDetails,
    enabled: bool,
}

#[derive(Debug, Clone)]
struct DesiredJobsRollback {
    stream_id: String,
    previous: Option<DesiredJobState>,
}

impl SchedulerJob {
    fn from_event(id: &str, details: v1::JobDetailsView<'_>) -> Self {
        let details = details.to_owned();
        let enabled = details.status() != v1::JobStatus::Disabled;
        Self {
            id: id.to_string(),
            details,
            enabled,
        }
    }

    fn pause(&mut self) {
        self.enabled = false;
        self.details.set_status(v1::JobStatus::Disabled);
    }

    fn resume(&mut self) {
        self.enabled = true;
        self.details.set_status(v1::JobStatus::Enabled);
    }

    fn resolve(&self) -> Result<ResolvedJob, CronError> {
        ResolvedJob::from_event(&self.id, protobuf::AsView::as_view(&self.details))
    }
}

impl PartialEq for SchedulerJob {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.enabled == other.enabled
            && job_details_eq(
                protobuf::AsView::as_view(&self.details),
                protobuf::AsView::as_view(&other.details),
            )
    }
}

fn job_details_eq(left: v1::JobDetailsView<'_>, right: v1::JobDetailsView<'_>) -> bool {
    left.has_status() == right.has_status()
        && left.status() == right.status()
        && left.has_schedule() == right.has_schedule()
        && job_schedule_eq(left.schedule(), right.schedule())
        && left.has_delivery() == right.has_delivery()
        && job_delivery_eq(left.delivery(), right.delivery())
        && left.has_message() == right.has_message()
        && job_message_eq(left.message(), right.message())
}

fn job_schedule_eq(left: v1::JobScheduleView<'_>, right: v1::JobScheduleView<'_>) -> bool {
    match (left.kind(), right.kind()) {
        (v1::job_schedule::KindOneof::At(left), v1::job_schedule::KindOneof::At(right)) => {
            left.has_at() == right.has_at() && left.at().to_string() == right.at().to_string()
        }
        (v1::job_schedule::KindOneof::Every(left), v1::job_schedule::KindOneof::Every(right)) => {
            left.has_every_sec() == right.has_every_sec() && left.every_sec() == right.every_sec()
        }
        (v1::job_schedule::KindOneof::Cron(left), v1::job_schedule::KindOneof::Cron(right)) => {
            left.has_expr() == right.has_expr()
                && left.expr().to_string() == right.expr().to_string()
                && left.has_timezone() == right.has_timezone()
                && left.timezone().to_string() == right.timezone().to_string()
        }
        (v1::job_schedule::KindOneof::not_set(_), v1::job_schedule::KindOneof::not_set(_)) => true,
        _ => false,
    }
}

fn job_delivery_eq(left: v1::JobDeliveryView<'_>, right: v1::JobDeliveryView<'_>) -> bool {
    match (left.kind(), right.kind()) {
        (v1::job_delivery::KindOneof::NatsEvent(left), v1::job_delivery::KindOneof::NatsEvent(right)) => {
            left.has_route() == right.has_route()
                && left.route().to_string() == right.route().to_string()
                && left.has_ttl_sec() == right.has_ttl_sec()
                && left.ttl_sec() == right.ttl_sec()
                && left.has_source() == right.has_source()
                && job_sampling_source_eq(left.source(), right.source())
        }
        (v1::job_delivery::KindOneof::not_set(_), v1::job_delivery::KindOneof::not_set(_)) => true,
        _ => false,
    }
}

fn job_sampling_source_eq(left: v1::JobSamplingSourceView<'_>, right: v1::JobSamplingSourceView<'_>) -> bool {
    match (left.kind(), right.kind()) {
        (
            v1::job_sampling_source::KindOneof::LatestFromSubject(left),
            v1::job_sampling_source::KindOneof::LatestFromSubject(right),
        ) => left.has_subject() == right.has_subject() && left.subject().to_string() == right.subject().to_string(),
        (v1::job_sampling_source::KindOneof::not_set(_), v1::job_sampling_source::KindOneof::not_set(_)) => true,
        _ => false,
    }
}

fn job_message_eq(left: v1::JobMessageView<'_>, right: v1::JobMessageView<'_>) -> bool {
    left.has_content() == right.has_content()
        && left.content().to_string() == right.content().to_string()
        && left.headers().len() == right.headers().len()
        && left.headers().iter().zip(right.headers().iter()).all(|(left, right)| {
            left.has_name() == right.has_name()
                && left.name().to_string() == right.name().to_string()
                && left.has_value() == right.has_value()
                && left.value().to_string() == right.value().to_string()
        })
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
                _ = signal::shutdown_signal() => {
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
                            let (change, rollback) = match handle_scheduler_message(&mut desired_jobs, &message).await {
                                Ok(result) => result,
                                Err(error) => {
                                    tracing::error!(error = %error, "Failed to apply scheduler event");
                                    nak_scheduler_message(&message).await;
                                    continue;
                                }
                            };
                            if let Err(error) = apply_scheduler_change(&self.schedule_publisher, &change).await {
                                tracing::error!(error = %error, "Failed to publish scheduler change");
                                rollback.restore(&mut desired_jobs);
                                nak_scheduler_message(&message).await;
                                continue;
                            }
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
            _ = signal::shutdown_signal() => {
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

    let consumer = stream
        .create_consumer(scheduler_replay_consumer_config(first_sequence))
        .await
        .map_err(|source| CronError::event_source("failed to create scheduler replay consumer", source))?;
    let mut messages = consumer
        .messages()
        .await
        .map_err(|source| CronError::event_source("failed to open scheduler replay stream", source))?;

    while let Some(message) = messages.next().await {
        let message =
            message.map_err(|source| CronError::event_source("failed to read scheduler replay event", source))?;
        let sequence = message
            .info()
            .map_err(|source| {
                CronError::event_source(
                    "failed to read scheduler replay event metadata",
                    std::io::Error::other(source.to_string()),
                )
            })?
            .stream_sequence;
        if sequence > last_sequence {
            break;
        }
        let reached_bootstrap_tail = sequence >= last_sequence;

        let event = decode_recorded_watch_message(&message)?;
        let stream_id = job_id_from_event_subject(event.stream_id())?;
        let data = event
            .decode_data_with(&JobEventCodec)
            .map_err(|source| CronError::event_source("failed to decode recorded job event payload", source))?;
        let _ = apply_scheduler_event(&mut desired_jobs, &stream_id, &data)?;
        if reached_bootstrap_tail {
            break;
        }
    }

    Ok(desired_jobs)
}

fn apply_scheduler_event(
    desired_jobs: &mut DesiredJobs,
    stream_id: &str,
    event: &v1::JobEvent,
) -> Result<SchedulerChange, CronError> {
    validate_event_job_id(stream_id).map_err(|source| {
        CronError::invalid_job_spec(crate::JobSpecError::InvalidId {
            id: stream_id.to_string(),
            source,
        })
    })?;

    match event.event() {
        v1::job_event::EventOneof::JobAdded(inner) => {
            if matches!(desired_jobs.get(stream_id), Some(DesiredJobState::Deleted)) {
                return Err(CronError::event_source(
                    "scheduler received an add event for a deleted job stream",
                    std::io::Error::other(stream_id.to_string()),
                ));
            }
            let job = SchedulerJob::from_event(stream_id, inner.job());
            desired_jobs.insert(job.id.clone(), DesiredJobState::Present(Box::new(job.clone())));
            Ok(SchedulerChange::Upsert(job))
        }
        v1::job_event::EventOneof::JobPaused(_) => {
            let job = desired_jobs.get_mut(stream_id).ok_or_else(|| {
                CronError::event_source(
                    "scheduler received a pause without current job state",
                    std::io::Error::other(stream_id.to_string()),
                )
            })?;
            match job {
                DesiredJobState::Present(job) => {
                    job.pause();
                    Ok(SchedulerChange::Delete(stream_id.to_string()))
                }
                DesiredJobState::Deleted => Err(CronError::event_source(
                    "scheduler received a pause for a deleted job stream",
                    std::io::Error::other(stream_id.to_string()),
                )),
            }
        }
        v1::job_event::EventOneof::JobResumed(_) => {
            let job = desired_jobs.get_mut(stream_id).ok_or_else(|| {
                CronError::event_source(
                    "scheduler received a resume without current job state",
                    std::io::Error::other(stream_id.to_string()),
                )
            })?;
            match job {
                DesiredJobState::Present(job) => {
                    job.resume();
                    Ok(SchedulerChange::Upsert(job.as_ref().clone()))
                }
                DesiredJobState::Deleted => Err(CronError::event_source(
                    "scheduler received a resume for a deleted job stream",
                    std::io::Error::other(stream_id.to_string()),
                )),
            }
        }
        v1::job_event::EventOneof::JobRemoved(_) => {
            desired_jobs.insert(stream_id.to_string(), DesiredJobState::Deleted);
            Ok(SchedulerChange::Delete(stream_id.to_string()))
        }
        _ => Err(CronError::event_source(
            "scheduler received an event without a supported case",
            std::io::Error::other("missing event case"),
        )),
    }
}

async fn handle_scheduler_message(
    desired_jobs: &mut DesiredJobs,
    message: &jetstream::Message,
) -> Result<(SchedulerChange, DesiredJobsRollback), CronError> {
    let event = decode_recorded_watch_message(message)?;
    let stream_id = job_id_from_event_subject(event.stream_id())?;
    let data = event
        .decode_data_with(&JobEventCodec)
        .map_err(|source| CronError::event_source("failed to decode watched scheduler event payload", source))?;
    let rollback = DesiredJobsRollback {
        stream_id: stream_id.clone(),
        previous: desired_jobs.get(&stream_id).cloned(),
    };
    match apply_scheduler_event(desired_jobs, &stream_id, &data) {
        Ok(change) => Ok((change, rollback)),
        Err(error) => {
            rollback.clone().restore(desired_jobs);
            Err(error)
        }
    }
}

impl DesiredJobsRollback {
    fn restore(self, desired_jobs: &mut DesiredJobs) {
        match self.previous {
            Some(previous) => {
                desired_jobs.insert(self.stream_id, previous);
            }
            None => {
                desired_jobs.remove(&self.stream_id);
            }
        }
    }
}

async fn apply_scheduler_change<P: SchedulePublisher<Error = CronError>>(
    publisher: &P,
    change: &SchedulerChange,
) -> Result<(), CronError> {
    match change {
        SchedulerChange::Upsert(job) => {
            if job.enabled {
                match job.resolve() {
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
        if !job.enabled {
            continue;
        }

        match job.resolve() {
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

    future::try_join_all(stale_ids.iter().map(|id| publisher.remove_schedule(id))).await?;
    future::try_join_all(resolved_jobs.iter().map(|resolved| publisher.upsert_schedule(resolved))).await?;

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

fn decode_recorded_job_event(
    message: async_nats::jetstream::message::StreamMessage,
) -> Result<RecordedEvent, CronError> {
    record_stream_message(message)
        .map_err(|source| CronError::event_source("failed to decode stored job event", source))
}

fn decode_recorded_watch_message(message: &async_nats::jetstream::Message) -> Result<RecordedEvent, CronError> {
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

fn scheduler_replay_consumer_config(start_sequence: u64) -> pull::OrderedConfig {
    pull::OrderedConfig {
        deliver_policy: DeliverPolicy::ByStartSequence { start_sequence },
        replay_policy: ReplayPolicy::Instant,
        ..Default::default()
    }
}

async fn ack_scheduler_message(message: &jetstream::Message) {
    if let Err(error) = message.ack().await {
        tracing::error!(error = %error, "Failed to acknowledge scheduler event");
    }
}

async fn nak_scheduler_message(message: &jetstream::Message) {
    if let Err(error) = message.ack_with(jetstream::AckKind::Nak(None)).await {
        tracing::error!(error = %error, "Failed to negatively acknowledge scheduler event");
    }
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
    use std::collections::HashMap;

    use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy};

    use super::{
        CronController, DesiredJobState, DesiredJobsRollback, SchedulerChange, SchedulerJob, apply_scheduler_change,
        apply_scheduler_event, default_leader_timing, next_scheduler_start_sequence, reconcile_snapshot,
        scheduler_consumer_config, scheduler_replay_consumer_config,
    };
    use crate::mocks::{MockCronStore, MockLeaderLock, MockSchedulePublisher};
    use crate::proto::v1;

    fn base_job_details() -> v1::JobDetails {
        let mut details = v1::JobDetails::new();
        details.set_status(v1::JobStatus::Enabled);
        details.set_schedule(every_schedule(30));
        details.set_delivery(nats_delivery(None));
        details.set_message(message(r#"{"kind":"heartbeat"}"#, []));
        details
    }

    fn expected_job(id: &str) -> SchedulerJob {
        let details = base_job_details();
        SchedulerJob::from_event(id, details.as_view())
    }

    fn disabled_job(id: &str) -> SchedulerJob {
        let mut details = base_job_details();
        details.set_status(v1::JobStatus::Disabled);
        SchedulerJob::from_event(id, details.as_view())
    }

    fn added_event(_id: &str) -> v1::JobEvent {
        let mut event = v1::JobEvent::new();
        let mut inner = v1::JobAdded::new();
        inner.set_job(base_job_details());
        event.set_job_added(inner);
        event
    }

    fn every_schedule(every_sec: u64) -> v1::JobSchedule {
        let mut schedule = v1::JobSchedule::new();
        let mut every = v1::EverySchedule::new();
        every.set_every_sec(every_sec);
        schedule.set_every(every);
        schedule
    }

    fn cron_schedule(expr: &str) -> v1::JobSchedule {
        let mut schedule = v1::JobSchedule::new();
        let mut cron = v1::CronSchedule::new();
        cron.set_expr(expr);
        schedule.set_cron(cron);
        schedule
    }

    fn nats_delivery(source: Option<&str>) -> v1::JobDelivery {
        let mut delivery = v1::JobDelivery::new();
        let mut nats = v1::NatsEventDelivery::new();
        nats.set_route("agent.run");
        if let Some(subject) = source {
            let mut source = v1::JobSamplingSource::new();
            let mut latest = v1::LatestFromSubjectSampling::new();
            latest.set_subject(subject);
            source.set_latest_from_subject(latest);
            nats.set_source(source);
        }
        delivery.set_nats_event(nats);
        delivery
    }

    fn message<const N: usize>(content: &str, headers: [(&str, &str); N]) -> v1::JobMessage {
        let mut message = v1::JobMessage::new();
        message.set_content(content);
        for (name, value) in headers {
            let mut header = v1::Header::new();
            header.set_name(name);
            header.set_value(value);
            message.headers_mut().push(header);
        }
        message
    }

    fn paused_event(_id: &str) -> v1::JobEvent {
        let mut event = v1::JobEvent::new();
        event.set_job_paused(v1::JobPaused::new());
        event
    }

    fn resumed_event(_id: &str) -> v1::JobEvent {
        let mut event = v1::JobEvent::new();
        event.set_job_resumed(v1::JobResumed::new());
        event
    }

    fn removed_event(_id: &str) -> v1::JobEvent {
        let mut event = v1::JobEvent::new();
        event.set_job_removed(v1::JobRemoved::new());
        event
    }

    fn invalid_enabled_job(id: &str) -> SchedulerJob {
        let mut details = base_job_details();
        details.set_schedule(cron_schedule("not-a-cron"));
        SchedulerJob::from_event(id, details.as_view())
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

        let desired_jobs = HashMap::from([(
            "disabled".to_string(),
            DesiredJobState::Present(Box::new(disabled_job("disabled"))),
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

        let added = apply_scheduler_event(&mut desired_jobs, "alpha", &added_event("alpha")).unwrap();
        assert_eq!(added, SchedulerChange::Upsert(expected_job("alpha")));
        assert!(matches!(desired_jobs.get("alpha"), Some(DesiredJobState::Present(_))));

        let disabled = apply_scheduler_event(&mut desired_jobs, "alpha", &paused_event("alpha")).unwrap();
        assert_eq!(disabled, SchedulerChange::Delete("alpha".to_string()));
        assert!(!match desired_jobs.get("alpha").unwrap() {
            DesiredJobState::Present(job) => job.enabled,
            DesiredJobState::Deleted => panic!("expected present job"),
        });

        let enabled = apply_scheduler_event(&mut desired_jobs, "alpha", &resumed_event("alpha")).unwrap();
        assert_eq!(enabled, SchedulerChange::Upsert(expected_job("alpha")));

        let removed = apply_scheduler_event(&mut desired_jobs, "alpha", &removed_event("alpha")).unwrap();
        assert_eq!(removed, SchedulerChange::Delete("alpha".to_string()));
        assert!(matches!(desired_jobs.get("alpha"), Some(DesiredJobState::Deleted)));

        let error = apply_scheduler_event(&mut desired_jobs, "alpha", &added_event("alpha")).unwrap_err();
        assert!(error.to_string().contains("deleted job stream"));
    }

    #[test]
    fn apply_scheduler_event_rejects_pause_without_current_job() {
        let mut desired_jobs = HashMap::new();
        let error = apply_scheduler_event(&mut desired_jobs, "missing", &paused_event("missing")).unwrap_err();

        assert!(error.to_string().contains("pause"));
    }

    #[test]
    fn desired_jobs_rollback_restores_single_touched_job() {
        let mut desired_jobs = HashMap::from([(
            "alpha".to_string(),
            DesiredJobState::Present(Box::new(expected_job("alpha"))),
        )]);
        let rollback = DesiredJobsRollback {
            stream_id: "alpha".to_string(),
            previous: desired_jobs.get("alpha").cloned(),
        };

        apply_scheduler_event(&mut desired_jobs, "alpha", &paused_event("alpha")).unwrap();
        rollback.restore(&mut desired_jobs);

        assert!(match desired_jobs.get("alpha").unwrap() {
            DesiredJobState::Present(job) => job.enabled,
            DesiredJobState::Deleted => false,
        });
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

        apply_scheduler_change(&publisher, &SchedulerChange::Upsert(disabled_job("disabled")))
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
        let replay_config = scheduler_replay_consumer_config(42);

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
        assert_eq!(
            replay_config.deliver_policy,
            DeliverPolicy::ByStartSequence { start_sequence: 42 }
        );
        assert_eq!(replay_config.replay_policy, ReplayPolicy::Instant);
    }
}
