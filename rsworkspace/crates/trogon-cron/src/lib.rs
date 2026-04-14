//! Generic scheduling control plane backed by native NATS scheduled messages.

pub mod commands;
pub mod config;
mod domain;
pub mod error;
pub mod events;
mod job_id;
pub mod kv;
pub mod nats;
pub mod scheduler;
pub mod store;
pub mod traits;

#[cfg(any(test, feature = "test-support"))]
pub mod mocks;

pub use commands::{
    ChangeJobStateCommand, ChangeJobStateState, GetJobCommand, ListJobsCommand,
    ReadRegisterJobCommandError, RegisterJobCommand, RegisterJobState, RemoveJobCommand,
    RemoveJobState, change_job_state, get_job, list_jobs,
    read_from_stdin as read_register_job_from_stdin, register_job, remove_job,
};
pub use config::{
    DeliverySpec, JobEnabledState, JobSpec, JobWriteCondition, SamplingSource, ScheduleSpec,
};
pub use domain::ResolvedJobSpec;
pub use error::{CronError, JobSpecError};
pub use events::{
    JobDecisionError, JobEvent, JobEventData, JobStreamState, JobTransitionError, ProjectionChange,
    RecordedJobEvent, apply, initial_state, projection_change,
};
pub use job_id::{JobId, JobIdError};
pub use nats::NatsSchedulePublisher;
pub use scheduler::CronController;
pub use store::{
    JobSpecChange, LoadAndWatchCommand, SNAPSHOT_STORE_CONFIG, append_events, connect_store,
    load_and_watch, open_snapshot_bucket,
};
pub use traits::{LeaderLock, SchedulePublisher};
