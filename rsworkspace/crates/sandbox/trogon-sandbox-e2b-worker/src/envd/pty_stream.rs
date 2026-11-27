use serde_json::Value;

use super::PtyEvent;
use super::constants::END_STREAM_FLAG;
use super::frame::decode_b64;
use super::frame_reader::FrameReader;
use super::process_stream::{exit_code_of, trailer_error};
use crate::api::ApiError;

/// A running PTY `Start` stream.
pub struct PtyStream {
    response: reqwest::Response,
    frames: FrameReader,
    finished: bool,
}

impl PtyStream {
    pub(super) fn new(response: reqwest::Response) -> Self {
        Self {
            response,
            frames: FrameReader::default(),
            finished: false,
        }
    }

    /// The next event, or `None` once the stream is over. A stream that ends with an error
    /// trailer reports it as `Err`.
    pub async fn next(&mut self) -> Result<Option<PtyEvent>, ApiError> {
        loop {
            if self.finished {
                return Ok(None);
            }
            while let Some((flags, payload)) = self.frames.next_frame()? {
                let value: Value = serde_json::from_slice(&payload)
                    .map_err(|error| ApiError::Transport(format!("unexpected envd frame: {error}")))?;
                if flags & END_STREAM_FLAG != 0 {
                    self.finished = true;
                    return match value.get("error") {
                        Some(error) => Err(trailer_error(error)),
                        None => Ok(None),
                    };
                }
                let Some(event) = value.get("event") else {
                    continue;
                };
                if let Some(pid) = event
                    .get("start")
                    .and_then(|start| start.get("pid"))
                    .and_then(Value::as_u64)
                {
                    return Ok(Some(PtyEvent::Started(pid as u32)));
                }
                if let Some(data) = event
                    .get("data")
                    .and_then(|data| data.get("pty"))
                    .and_then(Value::as_str)
                {
                    return Ok(Some(PtyEvent::Data(decode_b64(data))));
                }
                if let Some(end) = event.get("end") {
                    return Ok(Some(PtyEvent::Ended(exit_code_of(end))));
                }
            }
            match self.response.chunk().await.map_err(ApiError::transport)? {
                Some(chunk) => self.frames.push(&chunk),
                None => {
                    self.finished = true;
                    return Ok(None);
                }
            }
        }
    }
}
