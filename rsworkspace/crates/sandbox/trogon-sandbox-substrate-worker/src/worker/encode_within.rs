//! Encoding replies for a bus that carries at most `max_payload` bytes per message.
//!
//! A reply the bus refuses to publish leaves the caller waiting for a timeout, so a reply that
//! would not fit is trimmed or replaced by a typed denial instead.

use serde::Serialize;
use trogon_sandbox_contract::{
    CommandOutput, LeaseError, OutputTruncation, PollCommandResponse, ReadFileResponse, RunCommandResponse,
};
use trogon_sandbox_protocol::{COMMAND_REPLY_MARKER_ROOM, recap};

/// Encodes a run reply. Output that would not fit is trimmed to its head and tail and flagged
/// truncated. Only a reply that cannot fit even with empty output becomes a denial that keeps the
/// exit code.
pub(super) fn encode_run_within(response: RunCommandResponse, max_payload: usize) -> serde_json::Result<Vec<u8>> {
    let body = serde_json::to_vec(&response)?;
    match response {
        RunCommandResponse::Completed(output) if body.len() > max_payload => {
            let fitted = match fit_output(&output, max_payload, RunCommandResponse::Completed) {
                Some(fitted) => RunCommandResponse::Completed(fitted),
                None => RunCommandResponse::Denied(too_large_for(&output, max_payload)),
            };
            serde_json::to_vec(&fitted)
        }
        _ => Ok(body),
    }
}

/// Encodes a poll reply like [`encode_run_within`].
pub(super) fn encode_poll_within(response: PollCommandResponse, max_payload: usize) -> serde_json::Result<Vec<u8>> {
    let body = serde_json::to_vec(&response)?;
    match response {
        PollCommandResponse::Completed(output) if body.len() > max_payload => {
            let fitted = match fit_output(&output, max_payload, PollCommandResponse::Completed) {
                Some(fitted) => PollCommandResponse::Completed(fitted),
                None => PollCommandResponse::Denied(too_large_for(&output, max_payload)),
            };
            serde_json::to_vec(&fitted)
        }
        _ => Ok(body),
    }
}

/// Encodes a read reply. Content that would not fit is replaced by a denial naming the limit.
pub(super) fn encode_read_within(
    response: ReadFileResponse,
    path: &str,
    max_payload: usize,
) -> serde_json::Result<Vec<u8>> {
    let body = serde_json::to_vec(&response)?;
    match response {
        ReadFileResponse::Content { data } if body.len() > max_payload => {
            serde_json::to_vec(&ReadFileResponse::Denied(LeaseError::FileTooLargeForTransport {
                path: path.to_owned(),
                bytes: data.len() as u64,
                max_payload_bytes: max_payload as u64,
            }))
        }
        _ => Ok(body),
    }
}

/// Trims stdout and stderr, keeping the head and tail of each, until `wrap(output)` encodes
/// within `max_payload`. `None` when even empty streams would not fit.
fn fit_output<T: Serialize>(
    output: &CommandOutput,
    max_payload: usize,
    wrap: impl Fn(CommandOutput) -> T,
) -> Option<CommandOutput> {
    let encoded_len =
        |candidate: &CommandOutput| serde_json::to_vec(&wrap(candidate.clone())).map_or(usize::MAX, |body| body.len());
    // Only called for an output whose reply already exceeds `max_payload`, so it is not re-checked.
    let full = encoded_len(output);
    let empty = CommandOutput {
        stdout: Vec::new(),
        stderr: Vec::new(),
        truncated: OutputTruncation::Truncated,
        ..output.clone()
    };
    let empty_len = encoded_len(&empty);
    let envelope = empty_len.saturating_add(COMMAND_REPLY_MARKER_ROOM);
    let retained = output.stdout.len() + output.stderr.len();
    if envelope >= max_payload || retained == 0 {
        return None;
    }
    // Start from the measured cost per retained byte, then shrink until it fits.
    let per_byte = full.saturating_sub(empty_len) as f64 / retained as f64;
    let start = ((max_payload - envelope) as f64 / per_byte) as usize;
    // Shrink by a tenth until a candidate fits, trying an empty budget last.
    std::iter::successors(Some(start), |budget| (*budget > 0).then(|| budget * 9 / 10)).find_map(|budget| {
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
        (encoded_len(&candidate) <= max_payload).then_some(candidate)
    })
}

fn too_large_for(output: &CommandOutput, max_payload: usize) -> LeaseError {
    LeaseError::CommandOutputTooLargeForTransport {
        exit_code: output.exit_code,
        stdout_bytes: output.stdout.len() as u64,
        stderr_bytes: output.stderr.len() as u64,
        max_payload_bytes: max_payload as u64,
    }
}

/// Splits a retention budget between two streams: a stream that needs less than half keeps all
/// of it and the other gets the rest.
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
