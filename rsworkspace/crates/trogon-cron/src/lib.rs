#![cfg_attr(
    any(test, feature = "test-support"),
    allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]

//! Generic scheduling control plane backed by native NATS scheduled messages.

pub mod commands;
pub mod config;
pub mod error;
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
    CronExpression, Delivery, DeliveryRoute, EverySeconds, Job, JobHeaders, JobId, JobIdError, JobMessage, JobStatus,
    SamplingSource, SamplingSubject, Schedule, ScheduleTimezone, TtlSeconds,
};
pub use commands::event::{
    JobAdded, JobDetails, JobEvent, JobEventDelivery, JobEventSamplingSource, JobEventSchedule, JobEventStatus,
    JobPaused, JobRemoved, JobResumed, MessageContent, MessageEnvelope, MessageHeaders, MessageHeadersError,
};
pub use commands::{
    AddJobCommand, AddJobDecisionError, JobEventCodec, JobEventData, JobEventProtoError, JobState, JobStateProtoError,
    PauseJobCommand, PauseJobDecisionError, RecordedJobEvent, RemoveJobCommand, RemoveJobDecisionError,
    ResumeJobCommand, ResumeJobDecisionError, contract_v1,
};
pub use config::JobWriteCondition;
pub use error::{CronError, JobSpecError};
pub use nats::NatsSchedulePublisher;
pub use processors::CronController;
pub use projections::{
    CronJobChange, CronJobWatchStream, JobStreamState, JobTransitionError, LoadAndWatchCronJobsResult,
    ProjectionChange, apply, initial_state, load_and_watch_cron_jobs, projection_change,
};
pub use queries::{GetJobCommand, ListJobsCommand, get_job, list_jobs};
pub use read_model::CronJob;
pub use schedule::ResolvedJob;
pub use store::{Store, connect_store, open_snapshot_bucket, snapshot_store_config};
pub use traits::{LeaderLock, SchedulePublisher};
pub use trogon_eventsourcing::{CommandFailure, CommandInfraError, CommandResult, ExecutionResult, StreamState};
