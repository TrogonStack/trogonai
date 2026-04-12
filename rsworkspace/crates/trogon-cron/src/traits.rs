use futures::Stream;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;

pub use trogon_nats::lease::{ReleaseLease, RenewLease, TryAcquireLease};

use crate::{
    config::{JobEnabledState, JobSpec, JobWriteCondition, VersionedJobSpec},
    domain::ResolvedJobSpec,
    error::CronError,
};

pub trait LeaderLock:
    TryAcquireLease<Error = async_nats::jetstream::kv::CreateError>
    + RenewLease<Error = async_nats::jetstream::kv::UpdateError>
    + ReleaseLease<Error = async_nats::jetstream::kv::DeleteError>
    + Clone
    + Send
    + Sync
    + 'static
{
}

impl<T> LeaderLock for T where
    T: TryAcquireLease<Error = async_nats::jetstream::kv::CreateError>
        + RenewLease<Error = async_nats::jetstream::kv::UpdateError>
        + ReleaseLease<Error = async_nats::jetstream::kv::DeleteError>
        + Clone
        + Send
        + Sync
        + 'static
{
}

pub type ConfigWatchStream = Pin<Box<dyn Stream<Item = JobSpecChange> + Send + 'static>>;
pub type LoadAndWatchResult = Result<(Vec<JobSpec>, ConfigWatchStream), CronError>;

#[derive(Debug, Clone)]
pub enum JobSpecChange {
    Put(JobSpec),
    Delete(String),
}

pub trait ConfigStore: Clone + Send + Sync + 'static {
    fn put_job(
        &self,
        config: &JobSpec,
        write_condition: JobWriteCondition,
    ) -> impl Future<Output = Result<(), CronError>> + Send;

    fn set_job_state(
        &self,
        id: &str,
        state: JobEnabledState,
        write_condition: JobWriteCondition,
    ) -> impl Future<Output = Result<(), CronError>> + Send;

    fn get_job(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Option<VersionedJobSpec>, CronError>> + Send;

    fn delete_job(
        &self,
        id: &str,
        write_condition: JobWriteCondition,
    ) -> impl Future<Output = Result<(), CronError>> + Send;

    fn list_jobs(&self) -> impl Future<Output = Result<Vec<VersionedJobSpec>, CronError>> + Send;

    fn load_and_watch(&self) -> impl Future<Output = LoadAndWatchResult> + Send;
}

pub trait SchedulePublisher: Clone + Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    fn active_schedule_ids(
        &self,
    ) -> impl Future<Output = Result<HashSet<String>, Self::Error>> + Send;

    fn upsert_schedule(
        &self,
        job: &ResolvedJobSpec,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn remove_schedule(&self, job_id: &str)
    -> impl Future<Output = Result<(), Self::Error>> + Send;
}
