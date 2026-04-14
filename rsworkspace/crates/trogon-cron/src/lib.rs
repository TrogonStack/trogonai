//! Generic scheduling control plane backed by native NATS scheduled messages.

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
    ChangeJobStateCommand, ChangeJobStateState, GetJobCommand, JobSpecChange, ListJobsCommand,
    LoadAndWatchCommand, RegisterJobCommand, RegisterJobState, RemoveJobCommand, RemoveJobState,
    SNAPSHOT_STORE_CONFIG, append_events, change_job_state, connect_store, get_job, list_jobs,
    load_and_watch, open_snapshot_bucket, register_job, remove_job,
};
pub use traits::{LeaderLock, SchedulePublisher};
