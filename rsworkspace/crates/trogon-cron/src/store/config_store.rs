use futures::Stream;
use std::future::Future;
use std::pin::Pin;

use crate::{
    config::{JobSpec, VersionedJobSpec},
    error::CronError,
    store::{
        DeleteJobCommand, GetJobCommand, ListJobsCommand, LoadAndWatchCommand, PutJobCommand,
        SetJobStateCommand,
    },
};

pub type ConfigWatchStream = Pin<Box<dyn Stream<Item = JobSpecChange> + Send + 'static>>;
pub type LoadAndWatchResult = Result<(Vec<JobSpec>, ConfigWatchStream), CronError>;

#[derive(Debug, Clone)]
pub enum JobSpecChange {
    Put(JobSpec),
    Delete(String),
}

pub trait ConfigStore: Clone + Send + Sync + 'static {
    fn put_job(&self, command: PutJobCommand)
    -> impl Future<Output = Result<(), CronError>> + Send;

    fn set_job_state(
        &self,
        command: SetJobStateCommand,
    ) -> impl Future<Output = Result<(), CronError>> + Send;

    fn get_job(
        &self,
        command: GetJobCommand,
    ) -> impl Future<Output = Result<Option<VersionedJobSpec>, CronError>> + Send;

    fn delete_job(
        &self,
        command: DeleteJobCommand,
    ) -> impl Future<Output = Result<(), CronError>> + Send;

    fn list_jobs(
        &self,
        command: ListJobsCommand,
    ) -> impl Future<Output = Result<Vec<VersionedJobSpec>, CronError>> + Send;

    fn load_and_watch(
        &self,
        command: LoadAndWatchCommand,
    ) -> impl Future<Output = LoadAndWatchResult> + Send;
}
