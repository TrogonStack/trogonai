use trogon_sandbox_contract::{CommandExecution, CommandOutput, CommandTimeoutStatus, OutputStream, RunCommandRequest};
use trogon_sandbox_protocol::{cap_buffered, effective_max_output_bytes};

use crate::GuestOutcome;
use crate::constants::{CANCELLED_EXIT_CODE, EVAL_COMMAND};
use crate::session_command_error::SessionCommandError;

/// The JS source of a persistent-runtime command, or why the request cannot be honored.
pub(crate) fn session_command_source(request: &RunCommandRequest) -> Result<String, SessionCommandError> {
    if request.require_launch_cancellation.is_required() {
        return Err(SessionCommandError::LaunchCancellation);
    }
    if request.execution != CommandExecution::Attached
        || request.stream_output.is_enabled()
        || request.run_as_user.is_some()
    {
        return Err(SessionCommandError::NotForeground);
    }
    if request.cwd.is_some() || !request.env.is_empty() {
        return Err(SessionCommandError::CwdOrEnv);
    }
    match request.command.as_slice() {
        [verb, source] if verb == EVAL_COMMAND => Ok(source.clone()),
        _ => Err(SessionCommandError::Shape),
    }
}

/// The command output a persistent-runtime evaluation reports.
pub(crate) fn command_output(outcome: GuestOutcome, max_output_bytes: Option<u64>) -> CommandOutput {
    let (output, exit_code, timed_out, error) = match outcome {
        GuestOutcome::Completed { output } => (output, 0, CommandTimeoutStatus::WithinDeadline, None),
        GuestOutcome::Failed { output, error } => (output, 1, CommandTimeoutStatus::WithinDeadline, Some(error)),
        GuestOutcome::TimedOut { output } => (
            output,
            CommandOutput::TIMED_OUT_EXIT_CODE,
            CommandTimeoutStatus::TimedOut,
            None,
        ),
        GuestOutcome::Cancelled { output } => (
            output,
            CANCELLED_EXIT_CODE,
            CommandTimeoutStatus::WithinDeadline,
            Some("cancelled".to_owned()),
        ),
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    for (stream, text) in output.lines {
        match stream {
            OutputStream::Stdout => stdout.extend_from_slice(text.as_bytes()),
            OutputStream::Stderr => stderr.extend_from_slice(text.as_bytes()),
        }
    }
    if let Some(error) = error {
        stderr.extend_from_slice(error.as_bytes());
        stderr.push(b'\n');
    }
    let cap = effective_max_output_bytes(max_output_bytes, usize::MAX);
    let (stdout, out_truncated) = cap_buffered(stdout, cap);
    let (stderr, err_truncated) = cap_buffered(stderr, cap);
    CommandOutput {
        exit_code,
        stdout,
        stderr,
        timed_out,
        truncated: (out_truncated.is_truncated() || err_truncated.is_truncated()).into(),
    }
}
