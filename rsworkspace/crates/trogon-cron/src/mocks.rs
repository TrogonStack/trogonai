use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use async_nats::jetstream::kv;
use bytes::Bytes;
use futures::Stream;
use trogon_nats::lease::{ReleaseLease, RenewLease, TryAcquireLease};

use crate::{
    config::{JobEnabledState, JobSpec, JobWriteCondition, JobWriteState, VersionedJobSpec},
    domain::ResolvedJobSpec,
    domain::validate_job_spec,
    error::CronError,
    traits::{ConfigStore, JobSpecChange, SchedulePublisher},
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
pub struct MockConfigStore {
    jobs: Arc<Mutex<HashMap<String, VersionedJobSpec>>>,
    aggregate_versions: Arc<Mutex<HashMap<String, u64>>>,
}

impl MockConfigStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seed_job(&self, spec: JobSpec) {
        self.aggregate_versions
            .lock()
            .unwrap()
            .insert(spec.id.clone(), 1);
        self.jobs
            .lock()
            .unwrap()
            .insert(spec.id.clone(), VersionedJobSpec { version: 1, spec });
    }
}

impl ConfigStore for MockConfigStore {
    async fn put_job(
        &self,
        config: &JobSpec,
        write_condition: JobWriteCondition,
    ) -> Result<(), CronError> {
        validate_job_spec(config)?;
        let mut jobs = self.jobs.lock().unwrap();
        let mut aggregate_versions = self.aggregate_versions.lock().unwrap();
        let current_version = aggregate_versions.get(&config.id).copied();
        let write_state = JobWriteState::new(current_version, jobs.contains_key(&config.id));
        write_condition.ensure(&config.id, write_state)?;
        let next_version = current_version.unwrap_or(0) + 1;
        aggregate_versions.insert(config.id.clone(), next_version);
        jobs.insert(
            config.id.clone(),
            VersionedJobSpec {
                version: next_version,
                spec: config.clone(),
            },
        );
        Ok(())
    }

    async fn set_job_state(
        &self,
        id: &str,
        state: JobEnabledState,
        write_condition: JobWriteCondition,
    ) -> Result<(), CronError> {
        let mut jobs = self.jobs.lock().unwrap();
        let job = jobs
            .get_mut(id)
            .ok_or_else(|| CronError::JobNotFound { id: id.to_string() })?;
        write_condition.ensure(id, JobWriteState::new(Some(job.version), true))?;
        job.version += 1;
        job.spec.state = state;
        self.aggregate_versions
            .lock()
            .unwrap()
            .insert(id.to_string(), job.version);
        Ok(())
    }

    async fn get_job(&self, id: &str) -> Result<Option<VersionedJobSpec>, CronError> {
        Ok(self.jobs.lock().unwrap().get(id).cloned())
    }

    async fn delete_job(
        &self,
        id: &str,
        write_condition: JobWriteCondition,
    ) -> Result<(), CronError> {
        let mut jobs = self.jobs.lock().unwrap();
        let current_version = jobs
            .get(id)
            .ok_or_else(|| CronError::JobNotFound { id: id.to_string() })?
            .version;
        write_condition.ensure(id, JobWriteState::new(Some(current_version), true))?;
        self.aggregate_versions
            .lock()
            .unwrap()
            .insert(id.to_string(), current_version + 1);
        jobs.remove(id);
        Ok(())
    }

    async fn list_jobs(&self) -> Result<Vec<VersionedJobSpec>, CronError> {
        Ok(self.jobs.lock().unwrap().values().cloned().collect())
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
        let jobs = self
            .jobs
            .lock()
            .unwrap()
            .values()
            .cloned()
            .map(|job| job.spec)
            .collect();
        Ok((jobs, Box::pin(futures::stream::pending())))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config::{DeliverySpec, SamplingSource, ScheduleSpec};
    use futures::StreamExt;

    fn base_job(id: &str) -> JobSpec {
        JobSpec {
            id: id.to_string(),
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
    async fn mock_config_store_covers_crud_and_watch_snapshot() {
        let store = MockConfigStore::new();
        store.seed_job(base_job("seeded"));

        let seeded = store.get_job("seeded").await.unwrap().unwrap();
        assert_eq!(seeded.version, 1);

        store
            .put_job(&base_job("alpha"), JobWriteCondition::MustNotExist)
            .await
            .unwrap();
        let alpha = store.get_job("alpha").await.unwrap().unwrap();
        assert_eq!(alpha.version, 1);

        store
            .set_job_state(
                "alpha",
                JobEnabledState::Disabled,
                JobWriteCondition::MustBeAtVersion(alpha.version),
            )
            .await
            .unwrap();
        assert_eq!(
            store.get_job("alpha").await.unwrap().unwrap().spec.state,
            JobEnabledState::Disabled
        );

        let listed = store.list_jobs().await.unwrap();
        assert_eq!(listed.len(), 2);

        let (watch_jobs, mut watcher) = store.load_and_watch().await.unwrap();
        assert_eq!(watch_jobs.len(), 2);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(5), watcher.next())
                .await
                .is_err()
        );

        let alpha = store.get_job("alpha").await.unwrap().unwrap();
        store
            .delete_job("alpha", JobWriteCondition::MustBeAtVersion(alpha.version))
            .await
            .unwrap();
        assert!(store.get_job("alpha").await.unwrap().is_none());

        store
            .put_job(&base_job("alpha"), JobWriteCondition::MustNotExist)
            .await
            .unwrap();
        let re_registered = store.get_job("alpha").await.unwrap().unwrap();
        assert_eq!(re_registered.version, alpha.version + 2);
    }

    #[tokio::test]
    async fn mock_config_store_rejects_invalid_specs_and_stale_versions() {
        let store = MockConfigStore::new();
        let mut invalid = base_job("bad");
        invalid.delivery = DeliverySpec::NatsEvent {
            route: "agent.run".to_string(),
            headers: BTreeMap::new(),
            ttl_sec: None,
            source: Some(SamplingSource::LatestFromSubject {
                subject: "sensors.>".to_string(),
            }),
        };

        let invalid_error = store
            .put_job(&invalid, JobWriteCondition::MustNotExist)
            .await
            .unwrap_err();
        assert!(invalid_error.to_string().contains("sampling source"));

        store
            .put_job(&base_job("alpha"), JobWriteCondition::MustNotExist)
            .await
            .unwrap();
        let stale_error = store
            .set_job_state(
                "alpha",
                JobEnabledState::Disabled,
                JobWriteCondition::MustBeAtVersion(99),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            stale_error,
            CronError::OptimisticConcurrencyConflict { .. }
        ));

        let missing_error = store
            .set_job_state(
                "missing",
                JobEnabledState::Disabled,
                JobWriteCondition::MustNotExist,
            )
            .await
            .unwrap_err();
        assert!(matches!(missing_error, CronError::JobNotFound { .. }));
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
