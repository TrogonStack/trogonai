use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

use futures::{Stream, StreamExt};
use trogon_nats::lease::{
    LeaderElection, LeaseRenewInterval, LeaseTiming, LeaseTtl, NatsKvLease, NatsKvLeaseConfig,
};
use trogon_std::time::GetElapsed;
use uuid::Uuid;

use crate::{
    config::JobSpec,
    domain::ResolvedJobSpec,
    error::CronError,
    kv::{LEADER_BUCKET, LEADER_KEY},
    nats::NatsSchedulePublisher,
    store::{ConfigStore, JobSpecChange, LoadAndWatchCommand, connect_store},
    traits::{LeaderLock, SchedulePublisher},
};

const WATCH_RETRY_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_LEADER_RENEW_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_LEADER_TTL: Duration = Duration::from_secs(10);

type ConfigWatcher = Pin<Box<dyn Stream<Item = JobSpecChange> + Send + 'static>>;

enum ReestablishedWatch {
    Ready((Vec<JobSpec>, ConfigWatcher)),
    Shutdown,
}

pub struct CronController<
    C = async_nats::jetstream::Context,
    P = NatsSchedulePublisher,
    L = NatsKvLease,
> {
    config_store: C,
    schedule_publisher: P,
    leader_lock: L,
    node_id: String,
    leader_timing: LeaseTiming,
}

impl CronController<async_nats::jetstream::Context, NatsSchedulePublisher, NatsKvLease> {
    pub async fn from_nats(nats: async_nats::Client) -> Result<Self, CronError> {
        let js = async_nats::jetstream::new(nats.clone());
        let config_store = connect_store(nats.clone()).await?;
        let schedule_publisher = NatsSchedulePublisher::new(nats).await?;
        let leader_config = NatsKvLeaseConfig::new(
            LEADER_BUCKET,
            LEADER_KEY,
            LeaseTtl::from_secs(DEFAULT_LEADER_TTL.as_secs())
                .expect("default leader TTL must be valid"),
            LeaseRenewInterval::from_secs(DEFAULT_LEADER_RENEW_INTERVAL.as_secs())
                .expect("default leader renew interval must be valid"),
        )
        .map_err(|source| CronError::lease_source("invalid leader lease config", source))?;
        let leader_lock = NatsKvLease::provision(&js, &leader_config)
            .await
            .map_err(|source| {
                CronError::lease_source("failed to provision leader lease", source)
            })?;

        Ok(Self {
            config_store,
            schedule_publisher,
            leader_lock,
            node_id: Uuid::new_v4().to_string(),
            leader_timing: leader_config.timing(),
        })
    }
}

impl<C, P, L> CronController<C, P, L>
where
    C: ConfigStore,
    P: SchedulePublisher<Error = CronError>,
    L: LeaderLock,
{
    pub fn new(config_store: C, schedule_publisher: P, leader_lock: L) -> Self {
        Self {
            config_store,
            schedule_publisher,
            leader_lock,
            node_id: Uuid::new_v4().to_string(),
            leader_timing: default_leader_timing(),
        }
    }

    pub fn with_node_id(mut self, node_id: String) -> Self {
        self.node_id = node_id;
        self
    }

    pub async fn run(self) -> Result<(), CronError> {
        let (initial_jobs, mut config_watcher) = self
            .config_store
            .load_and_watch(LoadAndWatchCommand)
            .await?;
        let mut desired_jobs = to_job_map(initial_jobs);
        let mut leader =
            LeaderElection::new(self.leader_lock, self.node_id.clone(), self.leader_timing);
        let mut currently_leader = false;
        let mut heartbeat = tokio::time::interval(self.leader_timing.renew_interval() / 2);

        loop {
            tokio::select! {
                _ = acp_telemetry::signal::shutdown_signal() => {
                    tracing::info!("Shutdown signal received, releasing leader lease");
                    if let Err(error) = leader.release().await {
                        tracing::warn!(error = %error, "Failed to release leader lease");
                    }
                    break;
                }
                _ = heartbeat.tick() => {
                    handle_heartbeat(
                        &mut leader,
                        &self.node_id,
                        &self.schedule_publisher,
                        &desired_jobs,
                        &mut currently_leader,
                    )
                    .await?;
                }
                change = config_watcher.next() => {
                    match change {
                        Some(change) => {
                            apply_local_change(&mut desired_jobs, &change);
                            if currently_leader {
                                apply_remote_change(
                                    &self.schedule_publisher,
                                    &change,
                                )
                                .await?;
                            }
                        }
                        None => {
                            tracing::warn!(
                                retry_ms = WATCH_RETRY_INTERVAL.as_millis(),
                                "Config watcher ended, attempting to re-establish it"
                            );
                            match reestablish_config_watch(&self.config_store).await? {
                                ReestablishedWatch::Ready((jobs, watcher)) => {
                                    desired_jobs = to_job_map(jobs);
                                    config_watcher = watcher;
                                    if currently_leader {
                                        reconcile_snapshot(
                                            &self.schedule_publisher,
                                            &desired_jobs,
                                        )
                                        .await?;
                                    }
                                }
                                ReestablishedWatch::Shutdown => {
                                    tracing::info!("Shutdown received while re-establishing config watcher, releasing leader lease");
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

fn default_leader_timing() -> LeaseTiming {
    LeaseTiming::new(
        LeaseTtl::from_secs(DEFAULT_LEADER_TTL.as_secs())
            .expect("default leader TTL must be valid"),
        LeaseRenewInterval::from_secs(DEFAULT_LEADER_RENEW_INTERVAL.as_secs())
            .expect("default leader renew interval must be valid"),
    )
    .expect("default leader timing must be valid")
}

fn to_job_map(jobs: Vec<JobSpec>) -> HashMap<String, JobSpec> {
    jobs.into_iter().map(|job| (job.id.clone(), job)).collect()
}

fn apply_local_change(desired_jobs: &mut HashMap<String, JobSpec>, change: &JobSpecChange) {
    match change {
        JobSpecChange::Put(job) => {
            desired_jobs.insert(job.id.clone(), job.clone());
        }
        JobSpecChange::Delete(id) => {
            desired_jobs.remove(id);
        }
    }
}

async fn handle_heartbeat<P, L, C>(
    leader: &mut LeaderElection<L, C>,
    node_id: &str,
    publisher: &P,
    desired_jobs: &HashMap<String, JobSpec>,
    currently_leader: &mut bool,
) -> Result<(), CronError>
where
    P: SchedulePublisher<Error = CronError>,
    L: LeaderLock,
    C: GetElapsed,
{
    match leader.ensure_leader().await {
        Ok(is_leader) => {
            if is_leader && !*currently_leader {
                tracing::info!(node_id = %node_id, job_count = desired_jobs.len(), "Controller became leader");
                reconcile_snapshot(publisher, desired_jobs).await?;
            }
            *currently_leader = is_leader;
        }
        Err(source) => {
            tracing::warn!(
                error = %source,
                node_id = %node_id,
                was_leader = *currently_leader,
                "Controller leadership check failed, stepping down and retrying"
            );
            *currently_leader = false;
        }
    }

    Ok(())
}

async fn apply_remote_change<P: SchedulePublisher<Error = CronError>>(
    publisher: &P,
    change: &JobSpecChange,
) -> Result<(), CronError> {
    match change {
        JobSpecChange::Put(job) => {
            if job.state.is_enabled() {
                match ResolvedJobSpec::try_from(job) {
                    Ok(resolved) => {
                        publisher.upsert_schedule(&resolved).await?;
                    }
                    Err(error) => {
                        tracing::error!(
                            error = %error,
                            job_id = %job.id,
                            "Skipping invalid enabled job change and removing any existing schedule"
                        );
                        publisher.remove_schedule(&job.id).await?;
                    }
                }
            } else {
                publisher.remove_schedule(&job.id).await?;
            }
        }
        JobSpecChange::Delete(id) => {
            publisher.remove_schedule(id).await?;
        }
    }
    Ok(())
}

async fn reconcile_snapshot<P: SchedulePublisher<Error = CronError>>(
    publisher: &P,
    desired_jobs: &HashMap<String, JobSpec>,
) -> Result<(), CronError> {
    let mut desired_active_ids = std::collections::HashSet::new();
    let mut resolved_jobs = Vec::new();

    for job in desired_jobs.values() {
        if !job.state.is_enabled() {
            continue;
        }

        match ResolvedJobSpec::try_from(job) {
            Ok(resolved) => {
                desired_active_ids.insert(job.id.clone());
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

async fn reestablish_config_watch<C: ConfigStore>(
    config_store: &C,
) -> Result<ReestablishedWatch, CronError> {
    loop {
        tokio::select! {
            _ = acp_telemetry::signal::shutdown_signal() => {
                return Ok(ReestablishedWatch::Shutdown);
            }
            result = config_store.load_and_watch(LoadAndWatchCommand) => {
                match result {
                    Ok((jobs, watcher)) => return Ok(ReestablishedWatch::Ready((jobs, watcher))),
                    Err(error) => {
                        tracing::error!(
                            error = %error,
                            retry_ms = WATCH_RETRY_INTERVAL.as_millis(),
                            "Failed to re-establish config watcher"
                        );
                    }
                }
            }
        }

        tokio::time::sleep(WATCH_RETRY_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{
        CronController, apply_local_change, apply_remote_change, default_leader_timing,
        handle_heartbeat, reconcile_snapshot, to_job_map,
    };
    use crate::{
        config::{DeliverySpec, JobEnabledState, JobSpec, ScheduleSpec},
        mocks::{MockConfigStore, MockLeaderLock, MockSchedulePublisher},
        store::JobSpecChange,
    };
    use trogon_nats::lease::{LeaderElection, LeaseRenewInterval, LeaseTiming, LeaseTtl};
    use trogon_std::time::{GetElapsed, GetNow};

    #[derive(Clone, Default)]
    struct TestClock {
        now: Arc<Mutex<Duration>>,
    }

    impl TestClock {
        fn advance(&self, duration: Duration) {
            let mut now = self.now.lock().unwrap();
            *now += duration;
        }
    }

    impl GetNow for TestClock {
        type Instant = Duration;

        fn now(&self) -> Self::Instant {
            *self.now.lock().unwrap()
        }
    }

    impl GetElapsed for TestClock {
        fn elapsed(&self, since: Self::Instant) -> Duration {
            self.now().saturating_sub(since)
        }
    }

    fn base_job(id: &str) -> JobSpec {
        JobSpec {
            id: id.to_string(),
            state: crate::config::JobEnabledState::Enabled,
            schedule: ScheduleSpec::Every { every_sec: 30 },
            delivery: DeliverySpec::NatsEvent {
                route: "agent.run".to_string(),
                headers: BTreeMap::new(),
                ttl_sec: None,
                source: None,
            },
            payload: serde_json::json!({"kind": "heartbeat"}),
            metadata: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn reconcile_snapshot_removes_orphaned_schedules_from_previous_leader() {
        let publisher = MockSchedulePublisher::new();
        publisher.seed_active_job("orphan");
        publisher.seed_active_job("heartbeat");

        let desired_jobs = HashMap::from([("heartbeat".to_string(), base_job("heartbeat"))]);

        reconcile_snapshot(&publisher, &desired_jobs).await.unwrap();

        assert_eq!(publisher.removals(), vec!["orphan"]);
        assert_eq!(publisher.upserts(), vec!["cron.schedules.heartbeat"]);
    }

    #[tokio::test]
    async fn reconcile_snapshot_removes_disabled_jobs_without_resolution() {
        let publisher = MockSchedulePublisher::new();
        publisher.seed_active_job("disabled");

        let mut disabled = base_job("disabled");
        disabled.state = crate::config::JobEnabledState::Disabled;
        disabled.delivery = DeliverySpec::NatsEvent {
            route: "agent.>".to_string(),
            headers: BTreeMap::new(),
            ttl_sec: None,
            source: None,
        };
        let desired_jobs = HashMap::from([("disabled".to_string(), disabled)]);

        reconcile_snapshot(&publisher, &desired_jobs).await.unwrap();

        assert_eq!(publisher.removals(), vec!["disabled"]);
        assert!(publisher.upserts().is_empty());
    }

    #[test]
    fn controller_construction_and_helpers_set_expected_state() {
        let controller = CronController::new(
            MockConfigStore::new(),
            MockSchedulePublisher::new(),
            MockLeaderLock::new(),
        )
        .with_node_id("node-1".to_string());

        assert_eq!(controller.node_id, "node-1");
        assert_eq!(
            controller.leader_timing.ttl(),
            default_leader_timing().ttl()
        );
        assert_eq!(
            controller.leader_timing.renew_interval(),
            default_leader_timing().renew_interval()
        );
    }

    #[test]
    fn to_job_map_and_apply_local_change_track_snapshot_state() {
        let mut jobs = to_job_map(vec![base_job("alpha")]);
        assert_eq!(jobs.keys().cloned().collect::<Vec<_>>(), vec!["alpha"]);

        apply_local_change(&mut jobs, &JobSpecChange::Put(base_job("beta")));
        assert!(jobs.contains_key("beta"));

        apply_local_change(&mut jobs, &JobSpecChange::Delete("alpha".to_string()));
        assert!(!jobs.contains_key("alpha"));
    }

    #[tokio::test]
    async fn apply_remote_change_upserts_enabled_jobs() {
        let publisher = MockSchedulePublisher::new();

        apply_remote_change(&publisher, &JobSpecChange::Put(base_job("enabled")))
            .await
            .unwrap();

        assert_eq!(publisher.upserts(), vec!["cron.schedules.enabled"]);
        assert!(publisher.removals().is_empty());
    }

    #[tokio::test]
    async fn apply_remote_change_removes_disabled_and_deleted_jobs() {
        let publisher = MockSchedulePublisher::new();
        let mut disabled = base_job("disabled");
        disabled.state = JobEnabledState::Disabled;

        apply_remote_change(&publisher, &JobSpecChange::Put(disabled))
            .await
            .unwrap();
        apply_remote_change(&publisher, &JobSpecChange::Delete("deleted".to_string()))
            .await
            .unwrap();

        assert!(publisher.upserts().is_empty());
        assert_eq!(publisher.removals(), vec!["disabled", "deleted"]);
    }

    #[tokio::test]
    async fn apply_remote_change_skips_invalid_enabled_jobs_and_removes_existing_schedule() {
        let publisher = MockSchedulePublisher::new();
        publisher.seed_active_job("invalid");
        let mut invalid = base_job("invalid");
        invalid.delivery = DeliverySpec::NatsEvent {
            route: "agent.>".to_string(),
            headers: BTreeMap::new(),
            ttl_sec: None,
            source: None,
        };

        apply_remote_change(&publisher, &JobSpecChange::Put(invalid))
            .await
            .unwrap();

        assert!(publisher.upserts().is_empty());
        assert_eq!(publisher.removals(), vec!["invalid"]);
    }

    #[tokio::test]
    async fn reconcile_snapshot_skips_invalid_enabled_jobs_without_blocking_valid_ones() {
        let publisher = MockSchedulePublisher::new();
        publisher.seed_active_job("invalid");
        let mut invalid = base_job("invalid");
        invalid.delivery = DeliverySpec::NatsEvent {
            route: "agent.>".to_string(),
            headers: BTreeMap::new(),
            ttl_sec: None,
            source: None,
        };
        let desired_jobs = HashMap::from([
            ("valid".to_string(), base_job("valid")),
            ("invalid".to_string(), invalid),
        ]);

        reconcile_snapshot(&publisher, &desired_jobs).await.unwrap();

        assert_eq!(publisher.removals(), vec!["invalid"]);
        assert_eq!(publisher.upserts(), vec!["cron.schedules.valid"]);
    }

    #[tokio::test]
    async fn heartbeat_demotes_and_continues_on_renew_error() {
        let publisher = MockSchedulePublisher::new();
        let desired_jobs = HashMap::from([("valid".to_string(), base_job("valid"))]);
        let lock = MockLeaderLock::new();
        let clock = TestClock::default();
        let timing = LeaseTiming::new(
            LeaseTtl::from_secs(10).unwrap(),
            LeaseRenewInterval::from_secs(5).unwrap(),
        )
        .unwrap();
        let mut leader =
            LeaderElection::with_clock(lock.clone(), "node-1".to_string(), timing, clock.clone());
        let mut currently_leader = false;

        handle_heartbeat(
            &mut leader,
            "node-1",
            &publisher,
            &desired_jobs,
            &mut currently_leader,
        )
        .await
        .unwrap();

        assert!(currently_leader);
        assert_eq!(publisher.upserts(), vec!["cron.schedules.valid"]);

        clock.advance(Duration::from_secs(5));
        lock.set_allow_renew(false);

        handle_heartbeat(
            &mut leader,
            "node-1",
            &publisher,
            &desired_jobs,
            &mut currently_leader,
        )
        .await
        .unwrap();

        assert!(!currently_leader);
        assert_eq!(publisher.upserts(), vec!["cron.schedules.valid"]);
    }
}
