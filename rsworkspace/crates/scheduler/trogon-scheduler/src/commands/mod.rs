pub mod domain;
mod record_schedule_occurrence;
mod schedule_next_occurrence;
mod snapshot;
mod state;

pub use trogon_scheduler_domain::{
    CreateSchedule, CreateScheduleDecideError, EvolveError, PauseSchedule, PauseScheduleError, RemoveSchedule,
    RemoveScheduleError, ResumeSchedule, ResumeScheduleError,
};

pub use record_schedule_occurrence::{RecordScheduleOccurrence, RecordScheduleOccurrenceError};
pub use schedule_next_occurrence::{ScheduleNextOccurrence, ScheduleNextOccurrenceError};

use trogon_std::{NowV7, UuidV7Generator};

/// Mints the opaque identifier for a new schedule at the host boundary.
pub fn mint_schedule_id() -> domain::ScheduleId {
    UuidV7Generator.now_v7().into()
}

#[cfg(test)]
mod tests;
