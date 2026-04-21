use std::num::NonZeroU64;

use trogon_eventsourcing::FrequencySnapshot;

mod add_job;
pub mod domain;
mod pause_job;
mod remove_job;
mod resume_job;

const COMMAND_SNAPSHOT_EVERY: NonZeroU64 =
    NonZeroU64::new(32).expect("command snapshot cadence must be non-zero");

fn command_snapshot_policy() -> FrequencySnapshot {
    FrequencySnapshot::new(COMMAND_SNAPSHOT_EVERY)
}

pub use add_job::{AddJobCommand, AddJobDecisionError, AddJobState, add_job};
pub use pause_job::{
    PauseJobCommand, PauseJobDecisionError, PauseJobError, PauseJobState, pause_job,
};
pub use remove_job::{RemoveJobCommand, RemoveJobDecisionError, RemoveJobState, remove_job};
pub use resume_job::{
    ResumeJobCommand, ResumeJobDecisionError, ResumeJobError, ResumeJobState, resume_job,
};
