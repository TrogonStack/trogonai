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
pub use events::{JobEvent, JobEventData, ProjectionChange, RecordedJobEvent};
pub use job_id::{JobId, JobIdError};
pub use nats::NatsSchedulePublisher;
pub use scheduler::CronController;
pub use store::connect_store;
pub use store::{
    ConfigStore, DeleteJobCommand, GetJobCommand, JobSpecChange, ListJobsCommand,
    LoadAndWatchCommand, PutJobCommand, SetJobStateCommand,
};
pub use traits::{LeaderLock, SchedulePublisher};
