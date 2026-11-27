use trogon_sandbox_contract::{CommandExecution, RunCommandRequest, StdinMode};

/// Options envd cannot honor are refused rather than silently dropped.
pub(super) fn run_command_violation(request: &RunCommandRequest) -> Option<&'static str> {
    if request.command.is_empty() {
        return Some("command must not be empty");
    }
    if request.stream_output.is_enabled() {
        return Some("this worker cannot stream command output");
    }
    match &request.execution {
        CommandExecution::Detached {
            stdin: StdinMode::Interactive,
            ..
        } => Some("this worker cannot keep a command's stdin open"),
        CommandExecution::Detached { process: Some(_), .. } => {
            Some("named processes and readiness probes are not supported")
        }
        CommandExecution::Detached { .. } | CommandExecution::Attached => None,
    }
}
