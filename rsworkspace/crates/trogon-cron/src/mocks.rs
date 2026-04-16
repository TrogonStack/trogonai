use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use async_nats::jetstream::kv;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::{Serialize, de::DeserializeOwned};
use trogon_eventsourcing::{
    AppendOutcome, EventData, EventStore, ExpectedState, NonEmpty, RecordedEvent, Snapshot,
    SnapshotStore, SnapshotStoreConfig, StreamCommand,
};
use trogon_nats::lease::{ReleaseLease, RenewLease, TryAcquireLease};

use crate::{
    GetJobCommand, ListJobsCommand,
    config::{JobSpec, JobWriteCondition, JobWriteState},
    domain::ResolvedJobSpec,
    error::CronError,
    events::{JobEvent, JobEventData},
    projections::{CronJobWatchStream, LoadAndWatchCronJobsResult},
    traits::SchedulePublisher,
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

    async fn upsert_schedule(&self, job: &ResolvedJobSpec) -> Result<(), Self::Error> {
        self.upserts
            .lock()
            .unwrap()
            .push(job.schedule_subject().to_string());
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
    jobs: Arc<Mutex<HashMap<String, Snapshot<JobSpec>>>>,
    stream_versions: Arc<Mutex<HashMap<String, u64>>>,
    events: Arc<Mutex<HashMap<String, Vec<JobEventData>>>>,
    command_snapshots: Arc<Mutex<HashMap<String, HashMap<String, String>>>>,
}

impl MockCronStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seed_job(&self, spec: JobSpec) {
        let id = spec.id.to_string();
        self.stream_versions.lock().unwrap().insert(id.clone(), 1);
        self.events.lock().unwrap().insert(
            id.clone(),
            vec![
                JobEventData::new(JobEvent::JobRegistered {
                    id: id.clone(),
                    spec: crate::RegisteredJobSpec::from(&spec),
                })
                .unwrap(),
            ],
        );
        self.jobs.lock().unwrap().insert(id, Snapshot::new(1, spec));
    }

    pub(crate) fn read_command_snapshot<Payload>(
        &self,
        config: trogon_eventsourcing::SnapshotStoreConfig<'static>,
        stream_id: &crate::JobId,
    ) -> Result<Option<Snapshot<Payload>>, CronError>
    where
        Payload: DeserializeOwned,
    {
        self.command_snapshots
            .lock()
            .unwrap()
            .get(config.key_prefix())
            .and_then(|snapshots| snapshots.get(stream_id.as_str()).cloned())
            .map(|snapshot| serde_json::from_str(&snapshot))
            .transpose()
            .map_err(CronError::from)
    }

    pub async fn get_job(
        &self,
        command: GetJobCommand,
    ) -> Result<Option<Snapshot<JobSpec>>, CronError> {
        Ok(self
            .jobs
            .lock()
            .unwrap()
            .get(command.stream_id().as_str())
            .cloned())
    }

    pub async fn list_jobs(
        &self,
        _command: ListJobsCommand,
    ) -> Result<Vec<Snapshot<JobSpec>>, CronError> {
        Ok(self.jobs.lock().unwrap().values().cloned().collect())
    }

    pub async fn load_and_watch_cron_jobs(&self) -> LoadAndWatchCronJobsResult {
        let jobs = self
            .jobs
            .lock()
            .unwrap()
            .values()
            .cloned()
            .map(|job| job.payload)
            .collect();
        Ok((
            jobs,
            Box::pin(futures::stream::pending()) as CronJobWatchStream,
        ))
    }
}

impl EventStore<crate::JobId> for MockCronStore {
    type Error = CronError;

    async fn current_stream_version(
        &self,
        stream_id: &crate::JobId,
    ) -> Result<Option<u64>, Self::Error> {
        Ok(self
            .stream_versions
            .lock()
            .unwrap()
            .get(stream_id.as_str())
            .copied())
    }

    async fn read_stream_from(
        &self,
        stream_id: &crate::JobId,
        from_sequence: u64,
    ) -> Result<Vec<RecordedEvent>, Self::Error> {
        let stream_events = self
            .events
            .lock()
            .unwrap()
            .get(stream_id.as_str())
            .cloned()
            .unwrap_or_default();
        if from_sequence == 0 {
            return Ok(Vec::new());
        }

        let mut recorded = Vec::new();
        for (index, event) in stream_events.into_iter().enumerate() {
            let sequence = index as u64 + 1;
            if sequence < from_sequence {
                continue;
            }
            recorded.push(event.record(
                stream_id.to_string(),
                Some(sequence),
                Some(sequence),
                DateTime::<Utc>::from_timestamp(1_700_000_000 + sequence as i64, 0).ok_or_else(
                    || {
                        CronError::event_source(
                            "failed to build mocked recorded event timestamp",
                            std::io::Error::other(stream_id.to_string()),
                        )
                    },
                )?,
            ));
        }
        Ok(recorded)
    }

    async fn append_events(
        &self,
        stream_id: &crate::JobId,
        expected_state: ExpectedState,
        events: NonEmpty<EventData>,
    ) -> Result<AppendOutcome, Self::Error> {
        let stream_id = stream_id.clone();
        let jobs = self.jobs.clone();
        let stream_versions = self.stream_versions.clone();
        let event_log = self.events.clone();
        if events
            .iter()
            .any(|event| event.stream_id() != stream_id.as_str())
        {
            return Err(CronError::event_source(
                "failed to append mocked job event batch",
                std::io::Error::other(format!(
                    "batch contains events outside stream '{}'",
                    stream_id
                )),
            ));
        }

        let mut jobs = jobs.lock().unwrap();
        let mut stream_versions = stream_versions.lock().unwrap();
        let mut stream_events = event_log.lock().unwrap();

        let current_snapshot = jobs.get(stream_id.as_str()).cloned();
        let current_version = stream_versions.get(stream_id.as_str()).copied();
        let write_state = JobWriteState::new(current_version, current_snapshot.as_ref().is_some());
        match expected_state {
            ExpectedState::Any => {}
            ExpectedState::StreamExists if write_state.exists() => {}
            ExpectedState::StreamExists => {
                return Err(CronError::OptimisticConcurrencyConflict {
                    id: stream_id.to_string(),
                    expected: ExpectedState::StreamExists,
                    current_version,
                });
            }
            ExpectedState::NoStream => {
                JobWriteCondition::MustNotExist.ensure(stream_id.as_str(), write_state)?;
            }
            ExpectedState::StreamRevision(version) => {
                JobWriteCondition::MustBeAtVersion(version)
                    .ensure(stream_id.as_str(), write_state)?;
            }
        }

        let stored_events = stream_events.entry(stream_id.to_string()).or_default();
        let mut projected_snapshot = current_snapshot;
        let mut version = current_version.unwrap_or(0);
        let appended_events = events.len() as u64;

        for event_data in events {
            let event = event_data.decode_data::<JobEvent>().map_err(|source| {
                CronError::event_source("failed to decode mocked job event payload", source)
            })?;
            version += 1;
            stored_events.push(event_data);
            match event {
                JobEvent::JobRegistered { id, spec } => {
                    let job_id = crate::JobId::parse(&id).map_err(|source| {
                        CronError::event_source("failed to project mocked job registration", source)
                    })?;
                    projected_snapshot = Some(Snapshot::new(version, spec.into_job_spec(job_id)));
                }
                JobEvent::JobStateChanged { state, .. } => {
                    let mut snapshot = projected_snapshot.take().ok_or_else(|| {
                        CronError::event_source(
                            "failed to project mocked job state change without current snapshot",
                            std::io::Error::other(stream_id.to_string()),
                        )
                    })?;
                    snapshot.version = version;
                    snapshot.payload.state = state;
                    projected_snapshot = Some(snapshot);
                }
                JobEvent::JobRemoved { .. } => {
                    projected_snapshot = None;
                }
            }
        }

        stream_versions.insert(stream_id.to_string(), version);
        if let Some(snapshot) = projected_snapshot {
            jobs.insert(stream_id.to_string(), snapshot);
        } else {
            jobs.remove(stream_id.as_str());
        }
        Ok(AppendOutcome {
            next_expected_version: current_version.unwrap_or(0) + appended_events,
        })
    }
}

impl<Payload> SnapshotStore<Payload, crate::JobId> for MockCronStore
where
    Payload: Serialize + DeserializeOwned + Send,
{
    type Error = CronError;

    async fn load_snapshot(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &crate::JobId,
    ) -> Result<Option<Snapshot<Payload>>, Self::Error> {
        self.read_command_snapshot(config, stream_id)
    }

    async fn save_snapshot(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &crate::JobId,
        snapshot: Snapshot<Payload>,
    ) -> Result<(), Self::Error> {
        let snapshot = serde_json::to_string(&snapshot)?;
        self.command_snapshots
            .lock()
            .unwrap()
            .entry(config.key_prefix().to_string())
            .or_default()
            .insert(stream_id.to_string(), snapshot);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use trogon_eventsourcing::CommandFailure;

    use super::*;
    use crate::config::{
        DeliverySpec, JobEnabledState, JobWriteCondition, SamplingSource, ScheduleSpec,
    };
    use crate::{
        ChangeJobStateCommand, GetJobCommand, ListJobsCommand, OccPolicy, RegisterJobCommand,
        RemoveJobCommand, change_job_state, register_job, remove_job,
    };
    use futures::StreamExt;

    fn job_id(id: &str) -> crate::JobId {
        crate::JobId::parse(id).unwrap()
    }

    fn base_job(id: &str) -> JobSpec {
        JobSpec {
            id: job_id(id),
            state: JobEnabledState::Enabled,
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
    async fn mock_schedule_publisher_tracks_active_jobs() {
        let publisher = MockSchedulePublisher::new();
        publisher.seed_active_job("orphan");
        let resolved = ResolvedJobSpec::try_from(&base_job("alpha")).unwrap();

        let active = publisher.active_schedule_ids().await.unwrap();
        assert!(active.contains("orphan"));

        publisher.upsert_schedule(&resolved).await.unwrap();
        publisher.remove_schedule("orphan").await.unwrap();

        assert_eq!(publisher.upserts(), vec!["cron.schedules.alpha"]);
        assert_eq!(publisher.removals(), vec!["orphan"]);
        assert!(
            publisher
                .active_schedule_ids()
                .await
                .unwrap()
                .contains("alpha")
        );
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
    async fn mock_cron_store_covers_crud_and_watch_snapshot() {
        let store = MockCronStore::new();
        store.seed_job(base_job("seeded"));

        let seeded = store
            .get_job(GetJobCommand {
                id: job_id("seeded"),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(seeded.version, 1);

        register_job(
            &store,
            &store,
            RegisterJobCommand::new(base_job("alpha")).unwrap(),
            OccPolicy::CommandDefault,
        )
        .await
        .unwrap();
        let alpha = store
            .get_job(GetJobCommand {
                id: job_id("alpha"),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(alpha.version, 1);

        change_job_state(
            &store,
            &store,
            ChangeJobStateCommand::new(job_id("alpha"), JobEnabledState::Disabled),
            OccPolicy::CommandDefault,
        )
        .await
        .unwrap();
        assert_eq!(
            store
                .get_job(GetJobCommand {
                    id: job_id("alpha"),
                })
                .await
                .unwrap()
                .unwrap()
                .payload
                .state,
            JobEnabledState::Disabled
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

        let alpha = store
            .get_job(GetJobCommand {
                id: job_id("alpha"),
            })
            .await
            .unwrap()
            .unwrap();
        remove_job(
            &store,
            &store,
            RemoveJobCommand::new(job_id("alpha")),
            OccPolicy::CommandDefault,
        )
        .await
        .unwrap();
        assert!(
            store
                .get_job(GetJobCommand {
                    id: job_id("alpha"),
                })
                .await
                .unwrap()
                .is_none()
        );

        register_job(
            &store,
            &store,
            RegisterJobCommand::new(base_job("alpha")).unwrap(),
            OccPolicy::CommandDefault,
        )
        .await
        .unwrap();
        let re_registered = store
            .get_job(GetJobCommand {
                id: job_id("alpha"),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(re_registered.version, alpha.version + 2);
    }

    #[tokio::test]
    async fn mock_cron_store_rejects_invalid_specs_and_state_errors() {
        let store = MockCronStore::new();
        let mut invalid = base_job("bad");
        invalid.delivery = DeliverySpec::NatsEvent {
            route: "agent.run".to_string(),
            headers: BTreeMap::new(),
            ttl_sec: None,
            source: Some(SamplingSource::LatestFromSubject {
                subject: "sensors.>".to_string(),
            }),
        };

        let invalid_error = RegisterJobCommand::new(invalid).unwrap_err();
        assert!(invalid_error.to_string().contains("sampling source"));

        register_job(
            &store,
            &store,
            RegisterJobCommand::new(base_job("alpha")).unwrap(),
            OccPolicy::CommandDefault,
        )
        .await
        .unwrap();
        let same_state_error = change_job_state(
            &store,
            &store,
            ChangeJobStateCommand::new(job_id("alpha"), JobEnabledState::Enabled),
            OccPolicy::CommandDefault,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            same_state_error,
            CommandFailure::Domain(CronError::JobStateAlreadySet { .. })
        ));

        let missing_error = change_job_state(
            &store,
            &store,
            ChangeJobStateCommand::new(job_id("missing"), JobEnabledState::Disabled),
            OccPolicy::CommandDefault,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            missing_error,
            CommandFailure::Domain(CronError::JobNotFound { .. })
        ));
    }

    #[test]
    fn ensure_write_condition_covers_accept_and_conflict_paths() {
        JobWriteCondition::MustNotExist
            .ensure("alpha", JobWriteState::new(None, false))
            .unwrap();
        JobWriteCondition::MustNotExist
            .ensure("alpha", JobWriteState::new(Some(4), false))
            .unwrap();
        JobWriteCondition::MustBeAtVersion(3)
            .ensure("alpha", JobWriteState::new(Some(3), true))
            .unwrap();

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
}
