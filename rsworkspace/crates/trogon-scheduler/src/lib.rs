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
pub mod processor;
pub mod projections;
pub mod queries;
mod read_model;
mod schedule;
pub mod store;
pub(crate) mod telemetry;
pub mod traits;

#[cfg(any(test, feature = "test-support"))]
pub mod mocks;

pub use commands::{
    CreateSchedule, CreateScheduleDecideError, EvolveError, PauseSchedule, PauseScheduleError, RemoveSchedule,
    RemoveScheduleError, ResumeSchedule, ResumeScheduleError,
};
pub use config::ScheduleWriteCondition;
pub use error::{ScheduleSpecError, SchedulerError};
pub use nats::NatsSchedulePublisher;
pub use projections::{
    LoadAndWatchSchedulesResult, ProjectionChange, ScheduleChange, ScheduleStreamState, ScheduleTransitionError,
    ScheduleWatchStream, apply, initial_state, load_and_watch_schedules, projection_change,
};
pub use queries::{
    GetSchedule, GetScheduleCommand, ListSchedules, ListSchedulesCommand, ScheduleId, ScheduleIdError, get_schedule,
    list_schedules,
};
pub use read_model::{
    MessageContent, MessageEnvelope, MessageHeaders, MessageHeadersError, Schedule, ScheduleDetails,
    ScheduleEventDelivery, ScheduleEventSamplingSource, ScheduleEventSchedule, ScheduleEventStatus,
};
pub use schedule::ResolvedSchedule;
pub use store::{Store, connect_store, open_command_snapshot_bucket};
pub use traits::{LeaderLock, SchedulePublisher};
pub use trogon_decider_runtime::{CommandError, CommandResult, ExecutionResult, StreamWritePrecondition};
pub use trogonai_proto::scheduler::schedules::{
    DeliveryKind, ScheduleEventCase, ScheduleEventPayloadError, ScheduleKind, ScheduleStatusKind, SourceKind, state_v1,
    v1,
};
