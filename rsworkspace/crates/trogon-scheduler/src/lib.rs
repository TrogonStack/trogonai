#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod commands;
pub mod processor;
pub(crate) mod telemetry;

pub use commands::{
    CreateSchedule, CreateScheduleDecideError, PauseSchedule, PauseScheduleError, RecordScheduleOccurrence,
    RecordScheduleOccurrenceError, RemoveSchedule, RemoveScheduleError, ResumeSchedule, ResumeScheduleError,
    ScheduleNextOccurrence, ScheduleNextOccurrenceError,
};
