//! Generic scheduling control plane backed by native NATS scheduled messages.

pub mod commands;
pub mod config;
mod domain;
pub mod error;
pub mod events;
mod job_id;
pub mod kv;
pub mod nats;
mod processors;
pub mod projections;
pub mod store;
pub mod traits;

#[cfg(any(test, feature = "test-support"))]
pub mod mocks;

pub use commands::{
    ChangeJobStateCommand, ChangeJobStateDecisionError, ChangeJobStateState, GetJobCommand,
    ListJobsCommand, ReadRegisterJobCommandError, RegisterJobCommand, RegisterJobDecisionError,
    RegisterJobState, RemoveJobCommand, RemoveJobDecisionError, RemoveJobState, change_job_state,
    get_job, list_jobs, read_from_stdin as read_register_job_from_stdin, register_job, remove_job,
};
pub use config::{
    DeliverySpec, JobEnabledState, JobSpec, JobWriteCondition, SamplingSource, ScheduleSpec,
};
pub use domain::ResolvedJobSpec;
pub use error::{CronError, JobSpecError};
pub use events::{JobEvent, JobEventData, RecordedJobEvent, RegisteredJobSpec};
pub use job_id::{JobId, JobIdError};
pub use nats::NatsSchedulePublisher;
pub use processors::CronController;
pub use projections::{
    CronJobChange, CronJobWatchStream, JobStreamState, JobTransitionError,
    LoadAndWatchCronJobsResult, ProjectionChange, apply, initial_state, load_and_watch_cron_jobs,
    projection_change,
};
pub use store::{SNAPSHOT_STORE_CONFIG, append_events, connect_store, open_snapshot_bucket};
pub use traits::{LeaderLock, SchedulePublisher};
