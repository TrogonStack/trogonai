#![cfg_attr(
    any(test, feature = "test-support"),
    allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]

//! Generic scheduling control plane backed by native NATS scheduled messages.

pub mod commands;
pub mod config;
mod domain;
pub mod error;
pub mod events;
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
    ListJobsCommand, RegisterJobCommand, RegisterJobDecisionError, RegisterJobState,
    RemoveJobCommand, RemoveJobDecisionError, RemoveJobState, change_job_state, get_job, list_jobs,
    register_job, remove_job,
};
pub use config::{
    DeliverySpec, JobEnabledState, JobSpec, JobWriteCondition, SamplingSource, ScheduleSpec,
};
pub use domain::{JobId, JobIdError, ResolvedJobSpec};
pub use error::{CronError, JobSpecError};
pub use events::{JobEvent, JobEventData, RecordedJobEvent, RegisteredJobSpec};
pub use nats::NatsSchedulePublisher;
pub use processors::CronController;
pub use projections::{
    CronJobChange, CronJobWatchStream, JobStreamState, JobTransitionError,
    LoadAndWatchCronJobsResult, ProjectionChange, apply, initial_state, load_and_watch_cron_jobs,
    projection_change,
};
pub use store::{SNAPSHOT_STORE_CONFIG, Store, append_events, connect_store, open_snapshot_bucket};
pub use traits::{LeaderLock, SchedulePublisher};
pub use trogon_eventsourcing::OccPolicy;
