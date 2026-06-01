use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use async_nats::jetstream::{
    self,
    consumer::{AckPolicy, DeliverPolicy, ReplayPolicy, pull},
    context::ConsumerInfoErrorKind,
};
use futures::{Stream, StreamExt, future};
use trogon_decider_nats::record_stream_message;
use trogon_decider_runtime::StreamEvent;
use trogon_nats::SubjectTokenViolation;
use trogon_nats::lease::{LeaderElection, LeaseRenewInterval, LeaseTiming, LeaseTtl, NatsKvLease, NatsKvLeaseConfig};
use trogon_std::{NowV7, UuidV7Generator};

use crate::{
    ResolvedSchedule, ScheduleEventCase, ScheduleStatusKind,
    error::SchedulerError,
    kv::{EVENTS_SUBJECT_PREFIX, LEADER_BUCKET, LEADER_KEY},
    nats::NatsSchedulePublisher,
    store::{Store, connect_store},
    traits::{LeaderLock, SchedulePublisher},
    v1,
};

const WATCH_RETRY_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_LEADER_RENEW_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_LEADER_TTL: Duration = Duration::from_secs(10);
const SCHEDULER_CONSUMER_NAME: &str = "cron_scheduler";
const SCHEDULER_CONSUMER_INACTIVE_THRESHOLD: Duration = Duration::from_secs(30);

type DesiredSchedules = HashMap<String, DesiredScheduleState>;
type SchedulerEventWatcher = Pin<Box<dyn Stream<Item = Result<jetstream::Message, SchedulerError>> + Send + 'static>>;

enum ReestablishedProcessor {
    Ready((DesiredSchedules, SchedulerEventWatcher)),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq)]
enum SchedulerChange {
    Upsert(SchedulerSchedule),
    Delete(String),
}

#[derive(Debug, Clone, PartialEq)]
enum DesiredScheduleState {
    Present(Box<SchedulerSchedule>),
    Deleted,
}

#[derive(Debug, Clone)]
struct SchedulerSchedule {
    id: String,
    details: v1::ScheduleCreated,
    enabled: bool,
}

#[derive(Debug, Clone)]
struct DesiredSchedulesRollback {
    stream_id: String,
    previous: Option<DesiredScheduleState>,
}

impl SchedulerSchedule {
    fn from_event(id: &str, details: &v1::ScheduleCreated) -> Self {
        let details = details.clone();
        let is_paused = matches!(
            details.status.as_option().and_then(|s| s.kind.as_ref()),
            Some(ScheduleStatusKind::Paused(_))
        );
        let enabled = !is_paused;
        Self {
            id: id.to_string(),
            details,
            enabled,
        }
    }

    fn pause(&mut self) {
        self.enabled = false;
        self.details.status = buffa::MessageField::some(v1::ScheduleStatus {
            kind: Some(v1::schedule_status::Paused {}.into()),
        });
    }

    fn resume(&mut self) {
        self.enabled = true;
        self.details.status = buffa::MessageField::some(v1::ScheduleStatus {
            kind: Some(v1::schedule_status::Scheduled {}.into()),
        });
    }

    fn resolve(&self) -> Result<ResolvedSchedule, SchedulerError> {
        ResolvedSchedule::from_event(&self.id, &self.details)
    }
}

impl PartialEq for SchedulerSchedule {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.enabled == other.enabled && self.details == other.details
    }
}

pub struct SchedulerController<C = Store, P = NatsSchedulePublisher, L = NatsKvLease> {
    store: C,
    schedule_publisher: P,
    leader_lock: L,
    node_id: String,
    leader_timing: LeaseTiming,
}

impl SchedulerController<Store, NatsSchedulePublisher, NatsKvLease> {
    pub async fn from_nats(nats: async_nats::Client) -> Result<Self, SchedulerError> {
        let js = async_nats::jetstream::new(nats.clone());
        let store = connect_store(nats.clone()).await?;
        let schedule_publisher = NatsSchedulePublisher::new(nats).await?;
        let leader_timing = default_leader_timing()?;
        let leader_config = NatsKvLeaseConfig::new(
            LEADER_BUCKET,
            LEADER_KEY,
            LeaseTtl::from_secs(DEFAULT_LEADER_TTL.as_secs())
                .map_err(|source| SchedulerError::lease_source("invalid default leader TTL", source))?,
            LeaseRenewInterval::from_secs(DEFAULT_LEADER_RENEW_INTERVAL.as_secs())
                .map_err(|source| SchedulerError::lease_source("invalid default leader renew interval", source))?,
        )
        .map_err(|source| SchedulerError::lease_source("invalid leader lease config", source))?;
        let leader_lock = NatsKvLease::provision(&js, &leader_config)
            .await
            .map_err(|source| SchedulerError::lease_source("failed to provision leader lease", source))?;

        Ok(Self {
            store,
            schedule_publisher,
            leader_lock,
            node_id: UuidV7Generator.now_v7().to_string(),
            leader_timing,
        })
    }
}

impl<C, P, L> SchedulerController<C, P, L> {
    pub fn new(store: C, schedule_publisher: P, leader_lock: L) -> Result<Self, SchedulerError> {
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

impl<P, L> SchedulerController<Store, P, L>
where
    P: SchedulePublisher<Error = SchedulerError>,
    L: LeaderLock,
{
    pub async fn run(self) -> Result<(), SchedulerError> {
        self.run_until(std::future::pending::<()>()).await
    }

    pub async fn run_until<S>(self, shutdown: S) -> Result<(), SchedulerError>
    where
        S: Future<Output = ()>,
    {
        tokio::pin!(shutdown);

        let mut desired_schedules = DesiredSchedules::new();
        let mut scheduler_watcher = inactive_scheduler_watcher();
        let mut leader = LeaderElection::new(self.leader_lock, self.node_id.clone(), self.leader_timing);
        let mut currently_leader = false;
        let mut heartbeat = tokio::time::interval(self.leader_timing.renew_interval() / 2);

        loop {
            tokio::select! {
                _ = shutdown.as_mut() => {
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
                                desired_schedules = rebuilt_jobs;
                                scheduler_watcher = watcher;
                            } else if !is_leader && currently_leader {
                                tracing::info!(node_id = %self.node_id, "Controller lost leadership");
                                desired_schedules.clear();
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
                            desired_schedules.clear();
                            scheduler_watcher = inactive_scheduler_watcher();
                        }
                    }
                }
                message = scheduler_watcher.next() => {
                    match message {
                        Some(Ok(message)) => {
                            let scheduler_event = match handle_scheduler_message(&mut desired_schedules, &message).await {
                                Ok(result) => result,
                                Err(error) => {
                                    tracing::error!(error = %error, "Failed to apply scheduler event");
                                    nak_scheduler_message(&message).await;
                                    continue;
                                }
                            };
                            let Some((change, rollback)) = scheduler_event else {
                                ack_scheduler_message(&message).await;
                                continue;
                            };
                            if let Err(error) = apply_scheduler_change(&self.schedule_publisher, &change).await {
                                tracing::error!(error = %error, "Failed to publish scheduler change");
                                rollback.restore(&mut desired_schedules);
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
                            match reestablish_scheduler_processor(
                                &self.store,
                                &self.schedule_publisher,
                                &mut shutdown,
                            ).await? {
                                ReestablishedProcessor::Ready((jobs, watcher)) => {
                                    desired_schedules = jobs;
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
                            match reestablish_scheduler_processor(
                                &self.store,
                                &self.schedule_publisher,
                                &mut shutdown,
                            ).await? {
                                ReestablishedProcessor::Ready((jobs, watcher)) => {
                                    desired_schedules = jobs;
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

fn default_leader_timing() -> Result<LeaseTiming, SchedulerError> {
    let ttl = LeaseTtl::from_secs(DEFAULT_LEADER_TTL.as_secs())
        .map_err(|source| SchedulerError::lease_source("invalid default leader TTL", source))?;
    let renew_interval = LeaseRenewInterval::from_secs(DEFAULT_LEADER_RENEW_INTERVAL.as_secs())
        .map_err(|source| SchedulerError::lease_source("invalid default leader renew interval", source))?;

    LeaseTiming::new(ttl, renew_interval)
        .map_err(|source| SchedulerError::lease_source("invalid default leader timing", source))
}

async fn establish_scheduler_processor<P>(
    store: &Store,
    publisher: &P,
) -> Result<(DesiredSchedules, SchedulerEventWatcher), SchedulerError>
where
    P: SchedulePublisher<Error = SchedulerError>,
{
    let stream = store.event_store.events_stream().clone();
    let info = stream
        .get_info()
        .await
        .map_err(|source| SchedulerError::event_source("failed to query events stream info", source))?;
    let desired_schedules =
        rebuild_scheduler_state_from_stream(&stream, info.state.first_sequence, info.state.last_sequence).await?;
    reconcile_snapshot(publisher, &desired_schedules).await?;
    let watcher =
        open_scheduler_event_watcher(&stream, next_scheduler_start_sequence(info.state.last_sequence)).await?;
    Ok((desired_schedules, watcher))
}

async fn reestablish_scheduler_processor<P, S>(
    store: &Store,
    publisher: &P,
    shutdown: &mut Pin<&mut S>,
) -> Result<ReestablishedProcessor, SchedulerError>
where
    P: SchedulePublisher<Error = SchedulerError>,
    S: Future<Output = ()>,
{
    loop {
        tokio::select! {
            _ = shutdown.as_mut() => {
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

        tokio::select! {
            _ = shutdown.as_mut() => {
                return Ok(ReestablishedProcessor::Shutdown);
            }
            _ = tokio::time::sleep(WATCH_RETRY_INTERVAL) => {}
        }
    }
}

async fn rebuild_scheduler_state_from_stream(
    stream: &jetstream::stream::Stream,
    first_sequence: u64,
    last_sequence: u64,
) -> Result<DesiredSchedules, SchedulerError> {
    let mut desired_schedules = DesiredSchedules::new();
    if last_sequence == 0 || first_sequence == 0 || first_sequence > last_sequence {
        return Ok(desired_schedules);
    }

    let consumer = stream
        .create_consumer(scheduler_replay_consumer_config(first_sequence))
        .await
        .map_err(|source| SchedulerError::event_source("failed to create scheduler replay consumer", source))?;
    let mut messages = consumer
        .messages()
        .await
        .map_err(|source| SchedulerError::event_source("failed to open scheduler replay stream", source))?;

    while let Some(message) = messages.next().await {
        let message =
            message.map_err(|source| SchedulerError::event_source("failed to read scheduler replay event", source))?;
        let sequence = message
            .info()
            .map_err(|source| {
                SchedulerError::event_source(
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
        let data = event.decode::<v1::ScheduleEvent>().map_err(|source| {
            SchedulerError::event_source("failed to decode recorded schedule event payload", source)
        })?;
        let Some(data) = data.into_decoded() else {
            if reached_bootstrap_tail {
                break;
            }
            continue;
        };
        let stream_id = schedule_id_from_event_subject(event.stream_id())?;
        let _ = apply_scheduler_event(&mut desired_schedules, &stream_id, &data)?;
        if reached_bootstrap_tail {
            break;
        }
    }

    Ok(desired_schedules)
}

fn apply_scheduler_event(
    desired_schedules: &mut DesiredSchedules,
    stream_id: &str,
    event: &v1::ScheduleEvent,
) -> Result<SchedulerChange, SchedulerError> {
    validate_event_schedule_id(stream_id).map_err(|source| {
        SchedulerError::invalid_schedule_spec(crate::ScheduleSpecError::InvalidId {
            id: stream_id.to_string(),
            source,
        })
    })?;
    validate_scheduler_event_schedule_id(stream_id, event)?;

    match &event.event {
        Some(ScheduleEventCase::ScheduleCreated(inner)) => {
            if matches!(desired_schedules.get(stream_id), Some(DesiredScheduleState::Deleted)) {
                return Err(SchedulerError::event_source(
                    "scheduler received an add event for a deleted schedule stream",
                    std::io::Error::other(stream_id.to_string()),
                ));
            }
            let job = SchedulerSchedule::from_event(stream_id, inner);
            desired_schedules.insert(job.id.clone(), DesiredScheduleState::Present(Box::new(job.clone())));
            Ok(SchedulerChange::Upsert(job))
        }
        Some(ScheduleEventCase::SchedulePaused(_)) => {
            let job = desired_schedules.get_mut(stream_id).ok_or_else(|| {
                SchedulerError::event_source(
                    "scheduler received a pause without current schedule state",
                    std::io::Error::other(stream_id.to_string()),
                )
            })?;
            match job {
                DesiredScheduleState::Present(job) => {
                    job.pause();
                    Ok(SchedulerChange::Delete(stream_id.to_string()))
                }
                DesiredScheduleState::Deleted => Err(SchedulerError::event_source(
                    "scheduler received a pause for a deleted schedule stream",
                    std::io::Error::other(stream_id.to_string()),
                )),
            }
        }
        Some(ScheduleEventCase::ScheduleResumed(_)) => {
            let job = desired_schedules.get_mut(stream_id).ok_or_else(|| {
                SchedulerError::event_source(
                    "scheduler received a resume without current schedule state",
                    std::io::Error::other(stream_id.to_string()),
                )
            })?;
            match job {
                DesiredScheduleState::Present(job) => {
                    job.resume();
                    Ok(SchedulerChange::Upsert(job.as_ref().clone()))
                }
                DesiredScheduleState::Deleted => Err(SchedulerError::event_source(
                    "scheduler received a resume for a deleted schedule stream",
                    std::io::Error::other(stream_id.to_string()),
                )),
            }
        }
        Some(ScheduleEventCase::ScheduleRemoved(_)) => {
            desired_schedules.insert(stream_id.to_string(), DesiredScheduleState::Deleted);
            Ok(SchedulerChange::Delete(stream_id.to_string()))
        }
        None => Err(SchedulerError::event_source(
            "scheduler received an event without a supported case",
            std::io::Error::other("missing event case"),
        )),
    }
}

fn validate_scheduler_event_schedule_id(stream_id: &str, event: &v1::ScheduleEvent) -> Result<(), SchedulerError> {
    let Some(job_id) = scheduler_event_schedule_id(event) else {
        return Ok(());
    };
    validate_event_schedule_id(job_id).map_err(|source| {
        SchedulerError::invalid_schedule_spec(crate::ScheduleSpecError::InvalidId {
            id: job_id.to_string(),
            source,
        })
    })?;
    if job_id == stream_id {
        Ok(())
    } else {
        Err(SchedulerError::event_source(
            "scheduler event schedule id does not match stream id",
            std::io::Error::other(format!("{job_id} != {stream_id}")),
        ))
    }
}

fn scheduler_event_schedule_id(event: &v1::ScheduleEvent) -> Option<&str> {
    match &event.event {
        Some(ScheduleEventCase::ScheduleCreated(inner)) => Some(&inner.schedule_id),
        Some(ScheduleEventCase::SchedulePaused(inner)) => Some(&inner.schedule_id),
        Some(ScheduleEventCase::ScheduleResumed(inner)) => Some(&inner.schedule_id),
        Some(ScheduleEventCase::ScheduleRemoved(inner)) => Some(&inner.schedule_id),
        None => None,
    }
}

async fn handle_scheduler_message(
    desired_schedules: &mut DesiredSchedules,
    message: &jetstream::Message,
) -> Result<Option<(SchedulerChange, DesiredSchedulesRollback)>, SchedulerError> {
    let event = decode_recorded_watch_message(message)?;
    let data = event
        .decode::<v1::ScheduleEvent>()
        .map_err(|source| SchedulerError::event_source("failed to decode watched scheduler event payload", source))?;
    let Some(data) = data.into_decoded() else {
        return Ok(None);
    };
    let stream_id = schedule_id_from_event_subject(event.stream_id())?;
    let rollback = DesiredSchedulesRollback {
        stream_id: stream_id.clone(),
        previous: desired_schedules.get(&stream_id).cloned(),
    };
    match apply_scheduler_event(desired_schedules, &stream_id, &data) {
        Ok(change) => Ok(Some((change, rollback))),
        Err(error) => {
            rollback.clone().restore(desired_schedules);
            Err(error)
        }
    }
}

impl DesiredSchedulesRollback {
    fn restore(self, desired_schedules: &mut DesiredSchedules) {
        match self.previous {
            Some(previous) => {
                desired_schedules.insert(self.stream_id, previous);
            }
            None => {
                desired_schedules.remove(&self.stream_id);
            }
        }
    }
}

async fn apply_scheduler_change<P: SchedulePublisher<Error = SchedulerError>>(
    publisher: &P,
    change: &SchedulerChange,
) -> Result<(), SchedulerError> {
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

async fn reconcile_snapshot<P: SchedulePublisher<Error = SchedulerError>>(
    publisher: &P,
    desired_schedules: &DesiredSchedules,
) -> Result<(), SchedulerError> {
    let mut desired_active_ids = std::collections::HashSet::new();
    let mut resolved_jobs = Vec::new();

    for job in desired_schedules.values() {
        let DesiredScheduleState::Present(job) = job else {
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
    Box::pin(futures::stream::pending::<Result<jetstream::Message, SchedulerError>>())
}

async fn open_scheduler_event_watcher(
    stream: &jetstream::stream::Stream,
    start_sequence: u64,
) -> Result<SchedulerEventWatcher, SchedulerError> {
    recreate_scheduler_consumer(stream, start_sequence).await?;
    let consumer = stream
        .get_consumer::<pull::Config>(SCHEDULER_CONSUMER_NAME)
        .await
        .map_err(|source| {
            SchedulerError::event_source(
                "failed to open scheduler event consumer",
                std::io::Error::other(source.to_string()),
            )
        })?;
    let messages = consumer
        .messages()
        .await
        .map_err(|source| SchedulerError::event_source("failed to open scheduler event watch stream", source))?;

    Ok(Box::pin(messages.map(|result| {
        result.map_err(|source| SchedulerError::event_source("failed to read scheduler event from consumer", source))
    })))
}

async fn recreate_scheduler_consumer(
    stream: &jetstream::stream::Stream,
    start_sequence: u64,
) -> Result<(), SchedulerError> {
    match stream.consumer_info(SCHEDULER_CONSUMER_NAME).await {
        Ok(_) => {
            stream
                .delete_consumer(SCHEDULER_CONSUMER_NAME)
                .await
                .map_err(|source| {
                    SchedulerError::event_source("failed to delete existing scheduler consumer", source)
                })?;
        }
        Err(error) if matches!(error.kind(), ConsumerInfoErrorKind::NotFound) => {}
        Err(error) => {
            return Err(SchedulerError::event_source(
                "failed to query existing scheduler consumer",
                error,
            ));
        }
    }

    stream
        .create_consumer(scheduler_consumer_config(start_sequence))
        .await
        .map_err(|source| SchedulerError::event_source("failed to create scheduler event consumer", source))?;

    Ok(())
}

fn decode_recorded_job_event(
    message: async_nats::jetstream::message::StreamMessage,
) -> Result<StreamEvent, SchedulerError> {
    let stream_id = message.subject.to_string();
    record_stream_message(message, stream_id)
        .map_err(|source| SchedulerError::event_source("failed to decode stored schedule event", source))
}

fn decode_recorded_watch_message(message: &async_nats::jetstream::Message) -> Result<StreamEvent, SchedulerError> {
    let stream_message =
        async_nats::jetstream::message::StreamMessage::try_from(message.message.clone()).map_err(|source| {
            SchedulerError::event_source(
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

fn schedule_id_from_event_subject(subject: &str) -> Result<String, SchedulerError> {
    let raw_id = subject.strip_prefix(EVENTS_SUBJECT_PREFIX).ok_or_else(|| {
        SchedulerError::event_source(
            "failed to derive schedule stream id from event subject",
            std::io::Error::other(subject.to_string()),
        )
    })?;

    validate_event_schedule_id(raw_id)
        .map(|()| raw_id.to_string())
        .map_err(|source| {
            SchedulerError::invalid_schedule_spec(crate::ScheduleSpecError::InvalidId {
                id: raw_id.to_string(),
                source,
            })
        })
}

fn validate_event_schedule_id(id: &str) -> Result<(), SubjectTokenViolation> {
    trogon_nats::NatsToken::new(id).map(|_| ())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy};
    use buffa::MessageField;
    use buffa_types::google::protobuf::Duration;

    use super::{
        DesiredScheduleState, DesiredSchedulesRollback, SchedulerChange, SchedulerController, SchedulerSchedule,
        apply_scheduler_change, apply_scheduler_event, default_leader_timing, next_scheduler_start_sequence,
        reconcile_snapshot, scheduler_consumer_config, scheduler_replay_consumer_config,
    };
    use crate::mocks::{MockLeaderLock, MockSchedulePublisher, MockSchedulerStore};
    use crate::v1;

    fn base_schedule_details() -> v1::ScheduleCreated {
        v1::ScheduleCreated {
            schedule_id: "heartbeat".to_string(),
            status: MessageField::some(v1::ScheduleStatus {
                kind: Some(v1::schedule_status::Scheduled {}.into()),
            }),
            schedule: MessageField::some(every_schedule(30)),
            delivery: MessageField::some(nats_delivery(None)),
            message: MessageField::some(message(r#"{"kind":"heartbeat"}"#, [])),
        }
    }

    fn expected_schedule(id: &str) -> SchedulerSchedule {
        let mut details = base_schedule_details();
        details.schedule_id = id.to_string();
        SchedulerSchedule::from_event(id, &details)
    }

    fn disabled_schedule(id: &str) -> SchedulerSchedule {
        let mut details = base_schedule_details();
        details.schedule_id = id.to_string();
        details.status = MessageField::some(v1::ScheduleStatus {
            kind: Some(v1::schedule_status::Paused {}.into()),
        });
        SchedulerSchedule::from_event(id, &details)
    }

    fn added_event(id: &str) -> v1::ScheduleEvent {
        let mut details = base_schedule_details();
        details.schedule_id = id.to_string();
        v1::ScheduleEvent {
            event: Some(details.into()),
        }
    }

    fn every_schedule(every_sec: u64) -> v1::Schedule {
        v1::Schedule {
            kind: Some(
                v1::schedule::Every {
                    every: MessageField::some(Duration {
                        seconds: every_sec as i64,
                        ..Default::default()
                    }),
                }
                .into(),
            ),
        }
    }

    fn cron_schedule(expr: &str) -> v1::Schedule {
        v1::Schedule {
            kind: Some(
                v1::schedule::Cron {
                    expr: expr.to_string(),
                    timezone: MessageField::none(),
                }
                .into(),
            ),
        }
    }

    fn nats_delivery(source: Option<&str>) -> v1::Delivery {
        v1::Delivery {
            kind: Some(
                v1::delivery::NatsMessage {
                    subject: "agent.run".to_string(),
                    ttl: MessageField::none(),
                    source: source
                        .map(|subject| v1::delivery::nats_message::Source {
                            kind: Some(
                                v1::delivery::nats_message::LatestFromSubject {
                                    subject: subject.to_string(),
                                }
                                .into(),
                            ),
                        })
                        .map(MessageField::some)
                        .unwrap_or_else(MessageField::none),
                }
                .into(),
            ),
        }
    }

    fn message<const N: usize>(content: &str, headers: [(&str, &str); N]) -> v1::Message {
        v1::Message {
            content: MessageField::some(trogonai_proto::content::v1alpha1::Content {
                content_type: "application/json".to_string(),
                data: content.as_bytes().to_vec(),
            }),
            headers: headers
                .into_iter()
                .map(|(name, value)| v1::Header {
                    name: name.to_string(),
                    value: value.to_string(),
                })
                .collect(),
        }
    }

    fn paused_event(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::SchedulePaused {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
        }
    }

    fn resumed_event(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleResumed {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
        }
    }

    fn removed_event(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleRemoved {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
        }
    }

    fn invalid_enabled_schedule(id: &str) -> SchedulerSchedule {
        let mut details = base_schedule_details();
        details.schedule_id = id.to_string();
        details.schedule = MessageField::some(cron_schedule("not-a-cron"));
        SchedulerSchedule::from_event(id, &details)
    }

    #[tokio::test]
    async fn reconcile_snapshot_removes_orphaned_schedules_from_previous_leader() {
        let publisher = MockSchedulePublisher::new();
        publisher.seed_active_schedule("orphan");
        publisher.seed_active_schedule("heartbeat");

        let desired_schedules = HashMap::from([(
            "heartbeat".to_string(),
            DesiredScheduleState::Present(Box::new(expected_schedule("heartbeat"))),
        )]);

        reconcile_snapshot(&publisher, &desired_schedules).await.unwrap();

        assert_eq!(publisher.removals(), vec!["orphan"]);
        assert_eq!(publisher.upserts(), vec!["scheduler.schedules.heartbeat"]);
    }

    #[tokio::test]
    async fn reconcile_snapshot_removes_disabled_schedules_without_resolution() {
        let publisher = MockSchedulePublisher::new();
        publisher.seed_active_schedule("disabled");

        let desired_schedules = HashMap::from([(
            "disabled".to_string(),
            DesiredScheduleState::Present(Box::new(disabled_schedule("disabled"))),
        )]);

        reconcile_snapshot(&publisher, &desired_schedules).await.unwrap();

        assert_eq!(publisher.removals(), vec!["disabled"]);
        assert!(publisher.upserts().is_empty());
    }

    #[test]
    fn controller_construction_and_helpers_set_expected_state() {
        let controller = SchedulerController::new(
            MockSchedulerStore::new(),
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
    fn desired_schedules_map_tracks_snapshot_state() {
        let jobs = HashMap::from([(
            "alpha".to_string(),
            DesiredScheduleState::Present(Box::new(expected_schedule("alpha"))),
        )]);
        assert_eq!(jobs.keys().cloned().collect::<Vec<_>>(), vec!["alpha"]);
    }

    #[test]
    fn apply_scheduler_event_tracks_register_disable_enable_and_terminal_delete() {
        let mut desired_schedules = HashMap::new();

        let added = apply_scheduler_event(&mut desired_schedules, "alpha", &added_event("alpha")).unwrap();
        assert_eq!(added, SchedulerChange::Upsert(expected_schedule("alpha")));
        assert!(matches!(
            desired_schedules.get("alpha"),
            Some(DesiredScheduleState::Present(_))
        ));

        let disabled = apply_scheduler_event(&mut desired_schedules, "alpha", &paused_event("alpha")).unwrap();
        assert_eq!(disabled, SchedulerChange::Delete("alpha".to_string()));
        assert!(!match desired_schedules.get("alpha").unwrap() {
            DesiredScheduleState::Present(job) => job.enabled,
            DesiredScheduleState::Deleted => panic!("expected present job"),
        });

        let enabled = apply_scheduler_event(&mut desired_schedules, "alpha", &resumed_event("alpha")).unwrap();
        assert_eq!(enabled, SchedulerChange::Upsert(expected_schedule("alpha")));

        let removed = apply_scheduler_event(&mut desired_schedules, "alpha", &removed_event("alpha")).unwrap();
        assert_eq!(removed, SchedulerChange::Delete("alpha".to_string()));
        assert!(matches!(
            desired_schedules.get("alpha"),
            Some(DesiredScheduleState::Deleted)
        ));

        let error = apply_scheduler_event(&mut desired_schedules, "alpha", &added_event("alpha")).unwrap_err();
        assert!(error.to_string().contains("deleted schedule stream"));
    }

    #[test]
    fn apply_scheduler_event_rejects_pause_without_current_job() {
        let mut desired_schedules = HashMap::new();
        let error = apply_scheduler_event(&mut desired_schedules, "missing", &paused_event("missing")).unwrap_err();

        assert!(error.to_string().contains("pause"));
    }

    #[test]
    fn desired_schedules_rollback_restores_single_touched_job() {
        let mut desired_schedules = HashMap::from([(
            "alpha".to_string(),
            DesiredScheduleState::Present(Box::new(expected_schedule("alpha"))),
        )]);
        let rollback = DesiredSchedulesRollback {
            stream_id: "alpha".to_string(),
            previous: desired_schedules.get("alpha").cloned(),
        };

        apply_scheduler_event(&mut desired_schedules, "alpha", &paused_event("alpha")).unwrap();
        rollback.restore(&mut desired_schedules);

        assert!(match desired_schedules.get("alpha").unwrap() {
            DesiredScheduleState::Present(job) => job.enabled,
            DesiredScheduleState::Deleted => false,
        });
    }

    #[tokio::test]
    async fn apply_scheduler_change_upserts_enabled_jobs() {
        let publisher = MockSchedulePublisher::new();

        apply_scheduler_change(&publisher, &SchedulerChange::Upsert(expected_schedule("enabled")))
            .await
            .unwrap();

        assert_eq!(publisher.upserts(), vec!["scheduler.schedules.enabled"]);
        assert!(publisher.removals().is_empty());
    }

    #[tokio::test]
    async fn apply_scheduler_change_removes_disabled_and_deleted_jobs() {
        let publisher = MockSchedulePublisher::new();

        apply_scheduler_change(&publisher, &SchedulerChange::Upsert(disabled_schedule("disabled")))
            .await
            .unwrap();
        apply_scheduler_change(&publisher, &SchedulerChange::Delete("deleted".to_string()))
            .await
            .unwrap();

        assert!(publisher.upserts().is_empty());
        assert_eq!(publisher.removals(), vec!["disabled", "deleted"]);
    }

    #[tokio::test]
    async fn apply_scheduler_change_skips_invalid_enabled_schedules_and_removes_existing_schedule() {
        let publisher = MockSchedulePublisher::new();
        publisher.seed_active_schedule("invalid");

        apply_scheduler_change(
            &publisher,
            &SchedulerChange::Upsert(invalid_enabled_schedule("invalid")),
        )
        .await
        .unwrap();

        assert!(publisher.upserts().is_empty());
        assert_eq!(publisher.removals(), vec!["invalid"]);
    }

    #[tokio::test]
    async fn reconcile_snapshot_skips_invalid_enabled_schedules_without_blocking_valid_ones() {
        let publisher = MockSchedulePublisher::new();
        publisher.seed_active_schedule("invalid");
        let desired_schedules = HashMap::from([
            (
                "valid".to_string(),
                DesiredScheduleState::Present(Box::new(expected_schedule("valid"))),
            ),
            (
                "invalid".to_string(),
                DesiredScheduleState::Present(Box::new(invalid_enabled_schedule("invalid"))),
            ),
        ]);

        reconcile_snapshot(&publisher, &desired_schedules).await.unwrap();

        assert_eq!(publisher.removals(), vec!["invalid"]);
        assert_eq!(publisher.upserts(), vec!["scheduler.schedules.valid"]);
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
