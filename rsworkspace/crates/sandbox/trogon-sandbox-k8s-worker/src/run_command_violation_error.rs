use trogon_sandbox_contract::{CommandDeadline, CommandExecution, RunCommandRequest};

/// A `run_command` option this worker refuses rather than silently drops.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub(crate) enum RunCommandViolationError {
    #[error("launch cancellation is not supported by this provider")]
    LaunchCancellation,
    #[error("lease deadline is not supported by this provider")]
    LeaseDeadline,
    #[error("named processes are not supported by this provider")]
    Process,
}

impl RunCommandViolationError {
    /// The first option of `request` this worker cannot honor.
    pub(crate) fn of(request: &RunCommandRequest) -> Option<Self> {
        if request.require_launch_cancellation.is_required() {
            return Some(Self::LaunchCancellation);
        }
        match &request.execution {
            CommandExecution::Detached {
                deadline: CommandDeadline::Lease,
                ..
            } => Some(Self::LeaseDeadline),
            CommandExecution::Detached { process: Some(_), .. } => Some(Self::Process),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
