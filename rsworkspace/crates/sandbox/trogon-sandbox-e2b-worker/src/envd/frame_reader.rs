use super::constants::{FRAME_HEADER_BYTES, MAX_FRAME_BYTES};
use super::frame::frame_length;
use crate::api::ApiError;

/// Splits a Connect byte stream into frames as chunks arrive.
#[derive(Default)]
pub(super) struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    pub(super) fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    pub(super) fn next_frame(&mut self) -> Result<Option<(u8, Vec<u8>)>, ApiError> {
        if self.buf.len() < FRAME_HEADER_BYTES {
            return Ok(None);
        }
        let len = frame_length(&self.buf);
        if len > MAX_FRAME_BYTES {
            return Err(ApiError::Transport("envd frame exceeds the size limit".to_owned()));
        }
        let end = FRAME_HEADER_BYTES + len;
        if self.buf.len() < end {
            return Ok(None);
        }
        let flags = self.buf[0];
        let payload = self.buf[FRAME_HEADER_BYTES..end].to_vec();
        self.buf.drain(..end);
        Ok(Some((flags, payload)))
    }
}
