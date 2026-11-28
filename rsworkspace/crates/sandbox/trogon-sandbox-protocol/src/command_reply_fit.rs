//! Trimming finished command output so its reply fits one bus message.
//!
//! A reply the bus refuses to publish leaves the caller waiting for a timeout, so output that would not fit is cut
//! to the head and tail of each stream, and output that cannot fit even when empty becomes a typed denial.

use trogon_sandbox_contract::{CommandOutput, LeaseError, OutputTruncation, PollCommandResponse, RunCommandResponse};

use crate::constants::COMMAND_REPLY_MARKER_ROOM;
use crate::output_cap::recap;

/// The reply for a command, with finished output trimmed to fit one bus message; replies that carry no output pass
/// through unchanged.
pub fn fit_run_command_response(response: RunCommandResponse, max_payload: usize) -> RunCommandResponse {
    match response {
        RunCommandResponse::Completed(output) => {
            fit(output, max_payload).map_or_else(RunCommandResponse::Denied, RunCommandResponse::Completed)
        }
        other => other,
    }
}

/// The reply for a poll, trimmed the same way as [`fit_run_command_response`].
pub fn fit_poll_command_response(response: PollCommandResponse, max_payload: usize) -> PollCommandResponse {
    match response {
        PollCommandResponse::Completed(output) => {
            fit(output, max_payload).map_or_else(PollCommandResponse::Denied, PollCommandResponse::Completed)
        }
        other => other,
    }
}

/// Trims a finished command's output to one bus message, or says why it cannot be.
#[expect(
    clippy::result_large_err,
    reason = "LeaseError is the contract's wire error; boxing it here would only be unboxed by every caller"
)]
fn fit(output: CommandOutput, max_payload: usize) -> Result<CommandOutput, LeaseError> {
    fit_within(&output, max_payload).ok_or(LeaseError::CommandOutputTooLargeForTransport {
        exit_code: output.exit_code,
        stdout_bytes: output.stdout.len() as u64,
        stderr_bytes: output.stderr.len() as u64,
        max_payload_bytes: max_payload as u64,
    })
}

fn encoded_len(output: &CommandOutput) -> usize {
    serde_json::to_vec(&RunCommandResponse::Completed(output.clone())).map_or(usize::MAX, |body| body.len())
}

/// Trims stdout and stderr, keeping the head and tail of each, until the reply encodes within `max_payload`.
/// `None` when even empty streams would not fit.
fn fit_within(output: &CommandOutput, max_payload: usize) -> Option<CommandOutput> {
    let full = encoded_len(output);
    if full <= max_payload {
        return Some(output.clone());
    }
    let empty = CommandOutput {
        stdout: Vec::new(),
        stderr: Vec::new(),
        truncated: OutputTruncation::from(true),
        ..output.clone()
    };
    let empty_len = encoded_len(&empty);
    let envelope = empty_len.saturating_add(COMMAND_REPLY_MARKER_ROOM);
    let retained = output.stdout.len() + output.stderr.len();
    if envelope >= max_payload || retained == 0 {
        return None;
    }
    // Start from the measured cost per retained byte, then shrink until it fits.
    let growth = full.saturating_sub(empty_len).max(1);
    let mut budget = (max_payload - envelope).saturating_mul(retained) / growth;
    loop {
        let (stdout_cap, stderr_cap) = split_budget(budget, output.stdout.len(), output.stderr.len());
        let (stdout, stdout_cut) = recap(output.stdout.clone(), stdout_cap);
        let (stderr, stderr_cut) = recap(output.stderr.clone(), stderr_cap);
        let candidate = CommandOutput {
            stdout,
            stderr,
            truncated: OutputTruncation::from(
                output.truncated.is_truncated() || stdout_cut.is_truncated() || stderr_cut.is_truncated(),
            ),
            ..output.clone()
        };
        // The envelope check above leaves room for both markers, so a zero budget always fits.
        if budget == 0 || encoded_len(&candidate) <= max_payload {
            return Some(candidate);
        }
        budget = budget * 9 / 10;
    }
}

/// Splits a retention budget between two streams: a stream that needs less than half keeps all of it and the
/// other gets the rest.
fn split_budget(budget: usize, first: usize, second: usize) -> (usize, usize) {
    let half = budget / 2;
    if first <= half {
        (first, budget - first)
    } else if second <= budget - half {
        (budget - second, second)
    } else {
        (half, budget - half)
    }
}
