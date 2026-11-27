use trogon_sandbox_contract::{CommandDeadline, CommandExecution, LeaseError, RunCommandRequest};

use crate::lease_fault::invalid_spec;

/// A command line this agent can actually start: a program and its arguments.
///
/// Parsing the request into this is what makes "empty command" unrepresentable past the edge, instead of being
/// re-checked wherever the program is indexed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Launch<'a> {
    pub(crate) program: &'a str,
    pub(crate) args: &'a [String],
}

impl<'a> Launch<'a> {
    /// Refuses a request this provider cannot honour before anything is spawned.
    pub(crate) fn parse(request: &'a RunCommandRequest) -> Result<Self, LeaseError> {
        if request.require_launch_cancellation.is_required() {
            return Err(invalid_spec(
                "command launch cancellation is not supported by this provider".to_string(),
            ));
        }
        let Some((program, args)) = request.command.split_first() else {
            return Err(invalid_spec("command must not be empty".to_string()));
        };
        if request.timeout_seconds == 0 {
            return Err(invalid_spec("timeout_seconds must be > 0".to_string()));
        }
        if let CommandExecution::Detached {
            deadline: CommandDeadline::Lease,
            ..
        } = request.execution
        {
            return Err(invalid_spec(
                "lease deadline is not supported by this provider".to_string(),
            ));
        }
        Ok(Self { program, args })
    }
}

#[cfg(test)]
mod tests;
