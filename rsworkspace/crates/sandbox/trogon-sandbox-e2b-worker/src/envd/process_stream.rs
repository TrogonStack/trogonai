use serde_json::Value;

use super::ProcessResult;
use super::constants::{END_STREAM_FLAG, NO_CLEAN_EXIT, REJECTED_STATUS};
use super::frame::{decode_b64, split_frames};
use crate::api::ApiError;

/// The exit status of an `end` event. Protobuf JSON leaves zero values out, so a clean exit
/// has no `exitCode`.
pub(super) fn exit_code_of(end: &Value) -> i32 {
    let exited = end.get("exited").and_then(Value::as_bool).unwrap_or(false);
    match (exited, end.get("exitCode").and_then(Value::as_i64)) {
        (_, Some(code)) => code as i32,
        (true, None) => 0,
        (false, None) => NO_CLEAN_EXIT,
    }
}

/// The error a Connect trailer reports, by its code.
pub(super) fn trailer_error(error: &Value) -> ApiError {
    let code = error.get("code").and_then(Value::as_str).unwrap_or("unknown");
    let message = error.get("message").and_then(Value::as_str).unwrap_or("");
    match code {
        "unauthenticated" | "permission_denied" => ApiError::Unauthorized(message.to_owned()),
        "not_found" => ApiError::NotFound(message.to_owned()),
        _ => ApiError::Rejected {
            status: REJECTED_STATUS,
            message: format!("{code}: {message}"),
        },
    }
}

/// Whether a trailer only reports that the stream was cut short.
fn is_cut_short(error: &Value) -> bool {
    matches!(
        error.get("code").and_then(Value::as_str),
        Some("deadline_exceeded" | "canceled")
    )
}

/// Reads a finished Start stream into one result.
pub(crate) fn parse_process_stream(body: &[u8]) -> Result<ProcessResult, ApiError> {
    let mut result = ProcessResult::default();
    let mut ended = false;
    for (flags, payload) in split_frames(body)? {
        let value: Value = serde_json::from_slice(payload)
            .map_err(|error| ApiError::Transport(format!("unexpected envd frame: {error}")))?;
        if flags & END_STREAM_FLAG != 0 {
            match value.get("error") {
                Some(error) if is_cut_short(error) => {
                    if !ended {
                        return Err(ApiError::Transport(format!(
                            "envd {} before a native end event; execution termination is unconfirmed: {}",
                            error.get("code").and_then(Value::as_str).unwrap_or("unknown"),
                            error.get("message").and_then(Value::as_str).unwrap_or(""),
                        )));
                    }
                }
                Some(error) => return Err(trailer_error(error)),
                None => {}
            }
            continue;
        }
        let Some(event) = value.get("event") else {
            continue;
        };
        if let Some(data) = event.get("data") {
            if let Some(text) = data.get("stdout").and_then(Value::as_str) {
                result.stdout.extend(decode_b64(text)?);
            }
            if let Some(text) = data.get("stderr").and_then(Value::as_str) {
                result.stderr.extend(decode_b64(text)?);
            }
        }
        if let Some(end) = event.get("end") {
            ended = true;
            result.exit_code = exit_code_of(end);
        }
    }
    if !ended {
        return Err(ApiError::Transport(
            "the envd stream ended without an exit status".to_owned(),
        ));
    }
    Ok(result)
}
