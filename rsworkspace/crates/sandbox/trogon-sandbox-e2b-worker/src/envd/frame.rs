use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use serde::Serialize;

use super::constants::FRAME_HEADER_BYTES;
use crate::api::ApiError;

/// One Connect frame carrying `body` as JSON.
pub(super) fn encode_frame(body: &impl Serialize) -> Result<Vec<u8>, ApiError> {
    let body = serde_json::to_vec(body).map_err(|error| ApiError::Transport(error.to_string()))?;
    let length = u32::try_from(body.len()).map_err(|error| ApiError::Transport(error.to_string()))?;
    let mut out = Vec::with_capacity(body.len() + FRAME_HEADER_BYTES);
    out.push(0);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// The declared payload length of the frame whose header is `header`.
pub(super) fn frame_length(header: &[u8]) -> usize {
    u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize
}

/// The bytes a base64 stream chunk carries. A chunk that is not base64 fails the stream: reading it as empty would
/// drop output the command produced and still report a complete result.
pub(super) fn decode_b64(text: &str) -> Result<Vec<u8>, ApiError> {
    STANDARD
        .decode(text)
        .or_else(|_| STANDARD_NO_PAD.decode(text))
        .map_err(|error| ApiError::Transport(format!("envd sent output that is not base64: {error}")))
}

/// Splits a Connect response body into `(flags, payload)` frames.
pub(super) fn split_frames(mut body: &[u8]) -> Result<Vec<(u8, &[u8])>, ApiError> {
    let mut out = Vec::new();
    while !body.is_empty() {
        if body.len() < FRAME_HEADER_BYTES {
            return Err(truncated());
        }
        let end = FRAME_HEADER_BYTES + frame_length(body);
        let Some(payload) = body.get(FRAME_HEADER_BYTES..end) else {
            return Err(truncated());
        };
        out.push((body[0], payload));
        body = &body[end..];
    }
    Ok(out)
}

fn truncated() -> ApiError {
    ApiError::Transport("truncated envd frame".to_owned())
}
