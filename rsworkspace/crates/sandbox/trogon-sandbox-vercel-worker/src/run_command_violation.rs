use trogon_sandbox_contract::{CommandExecution, RunCommandRequest, StdinMode};

/// Options Vercel cannot honor are refused rather than silently dropped.
pub(crate) fn run_command_violation(request: &RunCommandRequest) -> Option<&'static str> {
    if request.require_launch_cancellation.is_required() {
        return Some("this worker cannot make a command launch cancelable");
    }
    if request.command.is_empty() {
        return Some("command must not be empty");
    }
    if request.stream_output.is_enabled() {
        return Some("this worker cannot stream command output");
    }
    if let CommandExecution::Detached { stdin, process, .. } = &request.execution {
        if *stdin == StdinMode::Interactive {
            return Some("this worker cannot keep a command's stdin open");
        }
        if process.is_some() {
            return Some("named processes and readiness probes are not supported");
        }
    }
    if let Some(user) = &request.run_as_user
        && user.as_str() != "root"
    {
        return Some("Vercel can run a command as the sandbox user or as root only");
    }
    None
}

#[cfg(test)]
mod tests;
