//! Generic scheduling control plane backed by native NATS scheduled messages.

pub mod config;
mod domain;
pub mod error;
pub mod events;
pub mod kv;
pub mod nats_impls;
pub mod scheduler;
pub mod traits;

#[cfg(any(test, feature = "test-support"))]
pub mod mocks;

pub use config::{
    DeliverySpec, JobEnabledState, JobSpec, JobWriteCondition, SamplingSource, ScheduleSpec,
    VersionedJobSpec,
};
pub use domain::ResolvedJobSpec;
pub use error::{CronError, JobSpecError};
pub use events::{JobEvent, ProjectionChange, RecordedJobEvent};
pub use nats_impls::{NatsConfigStore, NatsSchedulePublisher};
pub use scheduler::CronController;
pub use traits::{ConfigStore, JobSpecChange, LeaderLock, SchedulePublisher};
