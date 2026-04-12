mod append_events;
mod catch_up_snapshots;
mod config_bucket;
mod config_store;
mod connect;
mod delete_job;
mod events_stream;
mod get_job;
mod list_jobs;
mod list_snapshots;
mod load_and_watch;
mod load_snapshot;
mod project_event_to_snapshot;
mod put_job;
mod rewrite_projection;
mod set_job_state;
mod snapshot_bucket;

use async_nats::jetstream;

use crate::error::CronError;

pub use config_store::{ConfigStore, ConfigWatchStream, JobSpecChange, LoadAndWatchResult};
pub use connect::connect_store;
pub use delete_job::DeleteJobCommand;
pub use get_job::GetJobCommand;
pub use list_jobs::ListJobsCommand;
pub use load_and_watch::LoadAndWatchCommand;
pub use put_job::PutJobCommand;
pub use set_job_state::SetJobStateCommand;

impl ConfigStore for jetstream::Context {
    async fn put_job(&self, command: PutJobCommand) -> Result<(), CronError> {
        put_job::run(self, command).await
    }

    async fn set_job_state(&self, command: SetJobStateCommand) -> Result<(), CronError> {
        set_job_state::run(self, command).await
    }

    async fn get_job(
        &self,
        command: GetJobCommand,
    ) -> Result<Option<crate::VersionedJobSpec>, CronError> {
        get_job::run(self, command).await
    }

    async fn delete_job(&self, command: DeleteJobCommand) -> Result<(), CronError> {
        delete_job::run(self, command).await
    }

    async fn list_jobs(
        &self,
        command: ListJobsCommand,
    ) -> Result<Vec<crate::VersionedJobSpec>, CronError> {
        list_jobs::run(self, command).await
    }

    async fn load_and_watch(&self, command: LoadAndWatchCommand) -> LoadAndWatchResult {
        load_and_watch::run(self, command).await
    }
}
