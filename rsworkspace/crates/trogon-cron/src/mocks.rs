use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use async_nats::jetstream::kv;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::{Serialize, de::DeserializeOwned};
use trogon_eventsourcing::snapshot::{Snapshot, SnapshotStoreConfig};
use trogon_eventsourcing::{
    AppendOutcome, EventData, NonEmpty, SnapshotRead, SnapshotWrite, StreamAppend, StreamRead, StreamReadResult,
    StreamState,
};
use trogon_nats::lease::{ReleaseLease, RenewLease, TryAcquireLease};

use crate::{
    CronJob, GetJobCommand, ListJobsCommand, ResolvedJob,
    config::{JobWriteCondition, JobWriteState},
    error::CronError,
    events::{JobAdded, JobEvent, JobEventCodec, JobEventData, JobPaused, JobRemoved, JobResumed},
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
    jobs: Arc<Mutex<HashMap<String, Snapshot<CronJob>>>>,
    stream_versions: Arc<Mutex<HashMap<String, u64>>>,
    events: Arc<Mutex<HashMap<String, Vec<JobEventData>>>>,
    command_snapshots: Arc<Mutex<HashMap<String, HashMap<String, String>>>>,
}

impl MockCronStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seed_job(&self, job: crate::Job) {
        let id = job.id.to_string();
        self.stream_versions.lock().unwrap().insert(id.clone(), 1);
        self.events.lock().unwrap().insert(
            id.clone(),
            vec![
                JobEventData::new_with_codec(
                    &JobEventCodec,
                    JobEvent::JobAdded(JobAdded {
                        id: id.clone(),
                        job: crate::JobDetails::from(&job),
                    }),
                )
                .unwrap(),
            ],
        );
        self.jobs.lock().unwrap().insert(
            id.clone(),
            Snapshot::new(1, CronJob::from((id, crate::JobDetails::from(&job)))),
        );
    }

    pub(crate) fn read_command_snapshot<Payload>(
        &self,
        config: trogon_eventsourcing::SnapshotStoreConfig,
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

    pub async fn get_job(&self, command: GetJobCommand) -> Result<Option<CronJob>, CronError> {
        Ok(self
            .jobs
            .lock()
            .unwrap()
            .get(command.id.as_str())
            .cloned()
            .map(|job| job.payload))
    }

    pub async fn list_jobs(&self, _command: ListJobsCommand) -> Result<Vec<CronJob>, CronError> {
        Ok(self
            .jobs
            .lock()
            .unwrap()
            .values()
            .cloned()
            .map(|job| job.payload)
            .collect())
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
        Ok((jobs, Box::pin(futures::stream::pending()) as CronJobWatchStream))
    }
}

impl StreamRead<crate::JobId> for MockCronStore {
    type Error = CronError;

    async fn read_stream_from(
        &self,
        stream_id: &crate::JobId,
        from_sequence: u64,
    ) -> Result<StreamReadResult, Self::Error> {
        let current_version = self.stream_versions.lock().unwrap().get(stream_id.as_str()).copied();
        let stream_events = self
            .events
            .lock()
            .unwrap()
            .get(stream_id.as_str())
            .cloned()
            .unwrap_or_default();
        if from_sequence == 0 {
            return Ok(StreamReadResult {
                current_version,
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
                stream_id.to_string(),
                Some(sequence),
                Some(sequence),
                DateTime::<Utc>::from_timestamp(1_700_000_000 + sequence as i64, 0).ok_or_else(|| {
                    CronError::event_source(
                        "failed to build mocked recorded event timestamp",
                        std::io::Error::other(stream_id.to_string()),
                    )
                })?,
            ));
        }
        Ok(StreamReadResult {
            current_version,
            events: recorded,
        })
    }
}

impl StreamAppend<crate::JobId> for MockCronStore {
    type Error = CronError;

    async fn append_events(
        &self,
        stream_id: &crate::JobId,
        expected_state: StreamState,
        events: NonEmpty<EventData>,
    ) -> Result<AppendOutcome, Self::Error> {
        let stream_id = stream_id.clone();
        let jobs = self.jobs.clone();
        let stream_versions = self.stream_versions.clone();
        let event_log = self.events.clone();
        if events.iter().any(|event| event.stream_id() != stream_id.as_str()) {
            return Err(CronError::event_source(
                "failed to append mocked job event batch",
                std::io::Error::other(format!("batch contains events outside stream '{}'", stream_id)),
            ));
        }

        let mut jobs = jobs.lock().unwrap();
        let mut stream_versions = stream_versions.lock().unwrap();
        let mut stream_events = event_log.lock().unwrap();

        let current_snapshot = jobs.get(stream_id.as_str()).cloned();
        let current_version = stream_versions.get(stream_id.as_str()).copied();
        let write_state = JobWriteState::new(current_version, current_version.is_some());
        match expected_state {
            StreamState::Any => {}
            StreamState::StreamExists if write_state.exists() => {}
            StreamState::StreamExists => {
                return Err(CronError::OptimisticConcurrencyConflict {
                    id: stream_id.to_string(),
                    expected: StreamState::StreamExists,
                    current_version,
                });
            }
            StreamState::NoStream => {
                JobWriteCondition::MustNotExist.ensure(stream_id.as_str(), write_state)?;
            }
            StreamState::StreamRevision(version) => {
                JobWriteCondition::MustBeAtVersion(version).ensure(stream_id.as_str(), write_state)?;
            }
        }

        let stored_events = stream_events.entry(stream_id.to_string()).or_default();
        let mut projected_snapshot = current_snapshot;
        let mut version = current_version.unwrap_or(0);
        let appended_events = events.len() as u64;

        for event_data in events {
            let event = event_data
                .decode_data_with(&JobEventCodec)
                .map_err(|source| CronError::event_source("failed to decode mocked job event payload", source))?;
            version += 1;
            stored_events.push(event_data);
            match event {
                JobEvent::JobAdded(JobAdded { id, job }) => {
                    projected_snapshot = Some(Snapshot::new(version, CronJob::from((id, job))));
                }
                JobEvent::JobPaused(JobPaused { .. }) => {
                    let mut snapshot = projected_snapshot.take().ok_or_else(|| {
                        CronError::event_source(
                            "failed to project mocked job pause without current snapshot",
                            std::io::Error::other(stream_id.to_string()),
                        )
                    })?;
                    snapshot.version = version;
                    snapshot.payload.status = crate::JobEventStatus::Disabled;
                    projected_snapshot = Some(snapshot);
                }
                JobEvent::JobResumed(JobResumed { .. }) => {
                    let mut snapshot = projected_snapshot.take().ok_or_else(|| {
                        CronError::event_source(
                            "failed to project mocked job resume without current snapshot",
                            std::io::Error::other(stream_id.to_string()),
                        )
                    })?;
                    snapshot.version = version;
                    snapshot.payload.status = crate::JobEventStatus::Enabled;
                    projected_snapshot = Some(snapshot);
                }
                JobEvent::JobRemoved(JobRemoved { .. }) => {
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

impl<Payload> SnapshotRead<Payload, crate::JobId> for MockCronStore
where
    Payload: Serialize + DeserializeOwned + Send,
{
    type Error = CronError;

    async fn load_snapshot(
        &self,
        config: SnapshotStoreConfig,
        stream_id: &crate::JobId,
    ) -> Result<Option<Snapshot<Payload>>, Self::Error> {
        self.read_command_snapshot(config, stream_id)
    }
}

impl<Payload> SnapshotWrite<Payload, crate::JobId> for MockCronStore
where
    Payload: Serialize + DeserializeOwned + Send,
{
    type Error = CronError;

    async fn save_snapshot(
        &self,
        config: SnapshotStoreConfig,
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
    use trogon_eventsourcing::{CommandExecution, CommandFailure};

    use super::*;
    use crate::{
        AddJobCommand, CronJob, Delivery, GetJobCommand, Job, JobDetails, JobEventStatus, JobHeaders, JobId,
        JobMessage, JobStatus, JobWriteCondition, ListJobsCommand, MessageContent, PauseJobCommand, RemoveJobCommand,
        ResumeJobCommand, Schedule,
    };
    use futures::StreamExt;

    fn job_id(id: &str) -> crate::JobId {
        crate::JobId::parse(id).unwrap()
    }

    fn base_job(id: &str) -> Job {
        Job {
            id: job_id(id),
            status: JobStatus::Enabled,
            schedule: Schedule::every(30).unwrap(),
            delivery: Delivery::nats_event("agent.run").unwrap(),
            message: JobMessage {
                content: MessageContent::from_static(br#"{"kind":"heartbeat"}"#),
                headers: JobHeaders::default(),
            },
        }
    }

    fn expected_job(id: &str) -> CronJob {
        CronJob::from((id.to_string(), JobDetails::from(base_job(id))))
    }

    #[tokio::test]
    async fn mock_schedule_publisher_tracks_active_jobs() {
        let publisher = MockSchedulePublisher::new();
        publisher.seed_active_job("orphan");
        let resolved = ResolvedJob::try_from(&expected_job("alpha")).unwrap();

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
    async fn mock_cron_store_covers_crud_and_watch_snapshot() {
        let store = MockCronStore::new();
        store.seed_job(base_job("seeded"));

        let seeded = store
            .get_job(GetJobCommand::new(JobId::parse("seeded").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(seeded, expected_job("seeded"));

        CommandExecution::new(&store, &AddJobCommand::new(base_job("alpha")))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap();
        let alpha = store
            .get_job(GetJobCommand::new(JobId::parse("alpha").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(alpha, expected_job("alpha"));

        CommandExecution::new(&store, &PauseJobCommand::new(job_id("alpha")))
            .with_snapshot(&store)
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

        CommandExecution::new(&store, &RemoveJobCommand::new(job_id("alpha")))
            .with_snapshot(&store)
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

        let deleted_error = CommandExecution::new(&store, &AddJobCommand::new(base_job("alpha")))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap_err();
        assert!(matches!(
            deleted_error,
            CommandFailure::Decide(crate::AddJobDecisionError::JobDeleted { .. })
        ));
    }

    #[tokio::test]
    async fn mock_cron_store_rejects_invalid_specs_and_state_errors() {
        let store = MockCronStore::new();
        let invalid_error = serde_json::from_value::<Job>(serde_json::json!({
            "id": "bad",
            "schedule": { "type": "every", "every_sec": 30 },
            "delivery": {
                "type": "nats_event",
                "route": "agent.run",
                "source": { "type": "latest_from_subject", "subject": "sensors.>" }
            },
            "content": "eyJraW5kIjoiaGVhcnRiZWF0In0="
        }))
        .unwrap_err();
        assert!(invalid_error.to_string().contains("sampling source"));

        CommandExecution::new(&store, &AddJobCommand::new(base_job("alpha")))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap();
        let same_state_error = CommandExecution::new(&store, &ResumeJobCommand::new(job_id("alpha")))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap_err();
        assert!(matches!(
            same_state_error,
            CommandFailure::Decide(crate::ResumeJobDecisionError::AlreadyActive { .. })
        ));

        let missing_error = CommandExecution::new(&store, &PauseJobCommand::new(job_id("missing")))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap_err();
        assert!(matches!(
            missing_error,
            CommandFailure::Decide(crate::PauseJobDecisionError::JobNotFound { .. })
        ));
    }

    #[test]
    fn ensure_write_condition_covers_accept_and_conflict_paths() {
        JobWriteCondition::MustNotExist
            .ensure("alpha", JobWriteState::new(None, false))
            .unwrap();
        JobWriteCondition::MustBeAtVersion(3)
            .ensure("alpha", JobWriteState::new(Some(3), true))
            .unwrap();

        let error = JobWriteCondition::MustNotExist
            .ensure("alpha", JobWriteState::new(Some(4), true))
            .unwrap_err();
        assert!(matches!(
            error,
            CronError::OptimisticConcurrencyConflict {
                current_version: Some(4),
                ..
            }
        ));

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
