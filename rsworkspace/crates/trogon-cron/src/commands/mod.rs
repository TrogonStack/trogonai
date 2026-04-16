mod add;
mod get;
mod list;
mod remove;
mod set_state;

use trogon_eventsourcing::{CommandFailure, CommandInfraError, CommandOutcome};

use crate::{CronError, JobEvent};

pub type JobCommandResult =
    Result<CommandOutcome<JobEvent>, CommandFailure<CronError, CommandInfraError<CronError>>>;

pub use add::{
    RegisterJobCommand, RegisterJobDecisionError, RegisterJobState, run as register_job,
};
pub use get::{GetJobCommand, run as get_job};
pub use list::{ListJobsCommand, run as list_jobs};
pub use remove::{RemoveJobCommand, RemoveJobDecisionError, RemoveJobState, run as remove_job};
pub use set_state::{
    ChangeJobStateCommand, ChangeJobStateDecisionError, ChangeJobStateState,
    run as change_job_state,
};
