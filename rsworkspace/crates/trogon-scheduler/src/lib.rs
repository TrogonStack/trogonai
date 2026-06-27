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
mod projections;
pub mod queries;
#[cfg(not(coverage))]
pub mod store;
pub(crate) mod telemetry;

#[cfg(any(test, feature = "test-support"))]
pub mod mocks;

pub use commands::{
    CreateSchedule, CreateScheduleDecideError, EvolveError, PauseSchedule, PauseScheduleError,
    RecordScheduleOccurrence, RecordScheduleOccurrenceError, RemoveSchedule, RemoveScheduleError, ResumeSchedule,
    ResumeScheduleError, ScheduleNextOccurrence, ScheduleNextOccurrenceError,
};
pub use config::ScheduleWriteCondition;
pub use error::{ScheduleSpecError, SchedulerError};
pub use projections::storage::{SCHEDULES_BUCKET, SCHEDULES_CHECKPOINT_KEY};
pub use queries::read_model::{
    MessageContent, MessageEnvelope, MessageHeaders, MessageHeadersError, Schedule, ScheduleDetails,
    ScheduleEventDelivery, ScheduleEventSamplingSource, ScheduleEventSchedule, ScheduleEventStatus,
};
pub use queries::{GetSchedule, GetScheduleCommand, ListSchedules, ListSchedulesCommand, ScheduleId, ScheduleIdError};
#[cfg(not(coverage))]
pub use queries::{get_schedule, list_schedules};
#[cfg(not(coverage))]
pub use store::{Store, connect_store, open_command_snapshot_bucket};
pub use trogon_decider_runtime::{CommandError, CommandResult, ExecutionResult, StreamWritePrecondition};
pub use trogonai_proto::scheduler::schedules::{
    DeliveryKind, ScheduleEventCase, ScheduleEventPayloadError, ScheduleKind, ScheduleStatusKind, SourceKind,
    projections_v1, state_v1, v1,
};
