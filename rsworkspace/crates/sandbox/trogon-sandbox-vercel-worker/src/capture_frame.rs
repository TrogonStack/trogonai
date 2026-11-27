use trogon_sandbox_contract::{CommandDeadline, CommandOutput};
use trogon_sandbox_protocol::{GUEST_STREAM_LIMIT, parse_guest_capture};

use crate::VercelExecError;
use crate::collected::Collected;
use crate::execution::Execution;

/// The decoded size of a base64 field, or `None` when it is not canonical base64.
fn decoded_size(encoded: &str) -> Option<usize> {
    if !encoded.len().is_multiple_of(4) {
        return None;
    }
    let padding = encoded.bytes().rev().take_while(|byte| *byte == b'=').count();
    if padding > 2
        || !encoded.as_bytes()[..encoded.len() - padding]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'+' | b'/'))
    {
        return None;
    }
    Some(encoded.len() / 4 * 3 - padding)
}

/// Checks a capture frame is complete and turns it into the command's output.
pub(crate) fn captured_output(
    collected: Collected,
    execution: &Execution,
    carrier_exit: i32,
) -> Result<CommandOutput, VercelExecError> {
    if carrier_exit != 0
        || collected.truncated
        || collected.error.is_some()
        || !collected.stderr.into_bytes().is_empty()
    {
        return Err(VercelExecError::Capture(
            "native capture carrier failed or its frame is incomplete",
        ));
    }
    let bytes = collected.stdout.into_bytes();
    let frame = std::str::from_utf8(&bytes).map_err(|_| VercelExecError::Capture("capture frame is not ASCII"))?;
    let lines: Vec<_> = frame.splitn(9, '\n').collect();
    if !frame.is_ascii() || lines.len() != 8 || !lines[7].is_empty() {
        return Err(VercelExecError::Capture("capture frame has incomplete or extra fields"));
    }
    for stream in [1, 4] {
        let total = lines[stream]
            .trim()
            .parse::<u64>()
            .map_err(|_| VercelExecError::Capture("capture frame has invalid stream size"))?;
        let head = decoded_size(lines[stream + 1]);
        let tail = decoded_size(lines[stream + 2]);
        let expected = if total <= GUEST_STREAM_LIMIT as u64 {
            usize::try_from(total).ok().map(|total| (total, 0))
        } else {
            Some((GUEST_STREAM_LIMIT / 2, GUEST_STREAM_LIMIT / 2))
        };
        if head.zip(tail) != expected {
            return Err(VercelExecError::Capture("capture frame stream bytes are incomplete"));
        }
    }
    let output = parse_guest_capture(frame, execution.timeout_seconds, execution.max_output_bytes)
        .map_err(|_| VercelExecError::Capture("capture frame is malformed"))?;
    if output.timed_out.is_timed_out() && execution.deadline == CommandDeadline::Lease {
        return Err(VercelExecError::Capture(
            "lease-bound capture reported an unexpected finite deadline",
        ));
    }
    Ok(output)
}

#[cfg(test)]
mod tests;
