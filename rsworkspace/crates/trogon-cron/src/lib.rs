#![cfg_attr(
    any(test, feature = "test-support"),
    allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]

//! Generic scheduling control plane backed by native NATS scheduled messages.

pub mod commands;
pub mod config;
pub mod error;
pub mod events;
pub mod kv;
pub mod nats;
mod processors;
pub mod projections;
pub mod queries;
mod read_model;
mod schedule;
pub mod store;
pub mod traits;

#[cfg(any(test, feature = "test-support"))]
pub mod mocks;

pub use commands::domain::{
    CronExpression, DeliveryRoute, DeliverySpec, EverySeconds, JobEnabledState, JobHeaders, JobId,
    JobIdError, JobSpec, SamplingSource, SamplingSubject, ScheduleSpec, ScheduleTimezone,
    TtlSeconds,
};
pub use commands::{
    AddJobCommand, AddJobDecisionError, AddJobState, PauseJobCommand, PauseJobDecisionError,
    PauseJobError, PauseJobState, RemoveJobCommand, RemoveJobDecisionError, RemoveJobState,
    ResumeJobCommand, ResumeJobDecisionError, ResumeJobError, ResumeJobState, add_job, pause_job,
    remove_job, resume_job,
};
pub use config::JobWriteCondition;
pub use error::{CronError, JobSpecError};
pub use events::{
    JobAdded, JobDetails, JobEvent, JobEventCodec, JobEventData, JobEventDelivery,
    JobEventSamplingSource, JobEventSchedule, JobEventState, JobPaused, JobRemoved, JobResumed,
    MessageContent, MessageHeaders, MessageHeadersError, RecordedJobEvent,
};
pub use nats::NatsSchedulePublisher;
pub use processors::CronController;
pub use projections::{
    CronJobChange, CronJobWatchStream, JobStreamState, JobTransitionError,
    LoadAndWatchCronJobsResult, ProjectionChange, apply, initial_state, load_and_watch_cron_jobs,
    projection_change,
};
pub use queries::{GetJobCommand, ListJobsCommand, get_job, list_jobs};
pub use read_model::CronJob;
pub use schedule::ResolvedJobSpec;
pub use store::{Store, connect_store, open_snapshot_bucket, snapshot_store_config};
pub use traits::{LeaderLock, SchedulePublisher};
pub use trogon_eventsourcing::{
    CommandFailure, CommandInfraError, CommandResult, ExecutionResult, OccPolicy,
};
