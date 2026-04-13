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
    VersionedJobSpec,
};
pub use domain::ResolvedJobSpec;
pub use error::{CronError, JobSpecError};
pub use events::{
    JobEvent, JobEventData, JobStreamState, JobTransitionError, ProjectionChange, RecordedJobEvent,
    apply, initial_state, projection_change,
};
pub use job_id::{JobId, JobIdError};
pub use nats::NatsSchedulePublisher;
pub use scheduler::CronController;
pub use store::{
    DeleteJobCommand, GetJobCommand, JobSpecChange, ListJobsCommand, LoadAndWatchCommand,
    PutJobCommand, SNAPSHOT_STORE_CONFIG, SetJobStateCommand, append_events, connect_store,
    delete_job, get_job, list_jobs, load_and_watch, open_snapshot_bucket, put_job, set_job_state,
};
pub use traits::{LeaderLock, SchedulePublisher};
