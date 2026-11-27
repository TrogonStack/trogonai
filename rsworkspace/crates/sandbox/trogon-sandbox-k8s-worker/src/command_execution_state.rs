use trogon_sandbox_providers::PodExecOutput;

/// State of a detached command execution, tracked by `execution_id` until
/// polled to completion.
#[derive(Clone, Debug)]
pub(crate) enum CommandExecutionState {
    Running,
    Uncertain(String),
    Completed(PodExecOutput),
    Failed(String),
}
