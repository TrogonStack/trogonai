use trogon_sandbox_contract::{CommandLaunchOrigin, CommandOutput, ExecutionId};

/// The result of a `run_command` call, which is a completed run or a detached launch depending on the request.
pub enum RunCommandOutcome {
    Completed(CommandOutput),
    Started {
        execution_id: ExecutionId,
        origin: CommandLaunchOrigin,
    },
}
