//! Wasm-clean scheduler decider domain (commands + shared state).
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod commands;
#[cfg(feature = "runtime-snapshot")]
mod snapshot_policy;
mod subject;

pub use commands::CommandWireError;

pub use commands::domain;
pub use commands::{
    CreateSchedule, CreateScheduleDecideError, EvolveError, PauseSchedule, PauseScheduleError, RemoveSchedule,
    RemoveScheduleError, ResumeSchedule, ResumeScheduleError, evolve, initial_state,
};
