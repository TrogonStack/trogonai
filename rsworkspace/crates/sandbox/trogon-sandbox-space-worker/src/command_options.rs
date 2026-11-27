use trogon_sandbox_contract::{CommandExecution, RunCommandRequest};
use trogon_sandbox_protocol::resolve_exec_cwd;

use crate::CommandViolationError;

/// Options SPACE could serve but this worker does not wire are refused rather than
/// silently dropped.
pub(crate) fn run_command_violation(request: &RunCommandRequest) -> Option<CommandViolationError> {
    if request.require_launch_cancellation.is_required() {
        return Some(CommandViolationError::LaunchCancellation);
    }
    if request.command.is_empty() {
        return Some(CommandViolationError::EmptyCommand);
    }
    if !(1..=3600).contains(&request.timeout_seconds) {
        return Some(CommandViolationError::Timeout);
    }
    if request.execution != CommandExecution::Attached {
        return Some(CommandViolationError::Detached);
    }
    if request.stream_output.is_enabled() {
        return Some(CommandViolationError::StreamOutput);
    }
    if request.run_as_user.is_some() {
        return Some(CommandViolationError::RunAsUser);
    }
    None
}

pub(crate) fn resolve_cwd(
    base: Option<&str>,
    requested: Option<&str>,
) -> Result<Option<String>, CommandViolationError> {
    match (base, requested) {
        (_, Some(cwd)) if cwd.contains('\0') => Err(CommandViolationError::InvalidWorkingDirectory),
        (Some(base), requested) => Ok(Some(resolve_exec_cwd(base, requested))),
        (None, Some(cwd)) if cwd.starts_with('/') => Ok(Some(cwd.to_owned())),
        (None, Some(_)) => Err(CommandViolationError::RelativeWorkingDirectory),
        (None, None) => Ok(None),
    }
}
