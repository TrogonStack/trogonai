mod add;
mod get;
mod list;
mod remove;
mod runtime;
mod set_state;

use trogon_eventsourcing::ExpectedVersion;

use crate::JobWriteCondition;

pub(crate) const fn expected_version_from_write_condition(
    write_condition: JobWriteCondition,
) -> ExpectedVersion {
    match write_condition {
        JobWriteCondition::MustNotExist => ExpectedVersion::NoStream,
        JobWriteCondition::MustBeAtVersion(version) => ExpectedVersion::StreamVersion(version),
    }
}

pub use add::{
    RegisterJobCommand, RegisterJobDecisionError, RegisterJobState, run as register_job,
};
pub use get::{GetJobCommand, run as get_job};
pub use list::{ListJobsCommand, run as list_jobs};
pub use remove::{RemoveJobCommand, RemoveJobDecisionError, RemoveJobState, run as remove_job};
#[doc(hidden)]
pub use runtime::CronCommandExecutionRuntime;
pub(crate) use runtime::CronCommandRuntime;
pub use set_state::{
    ChangeJobStateCommand, ChangeJobStateDecisionError, ChangeJobStateState,
    run as change_job_state,
};
