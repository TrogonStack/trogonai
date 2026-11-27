use serde_json::Value;

use crate::constants::MAX_PENDING_BYTES;
use crate::{SseError, SseFrame};

/// Incremental parser for the Server-Sent Events SPACE sends on a command's data-plane
/// stream. Only `data:` and `id:` fields matter; comments, `event:` and `retry:` lines
/// are ignored.
#[derive(Debug, Default)]
pub struct SseParser {
    line: Vec<u8>,
    data: Vec<Vec<u8>>,
    id: Option<String>,
    skip_lf: bool,
}

impl SseParser {
    /// Feeds the next bytes of the stream and returns the events they complete.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<SseFrame>, SseError> {
        let mut frames = Vec::new();
        for &byte in chunk {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\r' => {
                    self.skip_lf = true;
                    self.end_line(&mut frames)?;
                }
                b'\n' => self.end_line(&mut frames)?,
                _ => {
                    self.line.push(byte);
                    if self.line.len() + self.data.iter().map(Vec::len).sum::<usize>() > MAX_PENDING_BYTES {
                        return Err(SseError::FrameTooLarge);
                    }
                }
            }
        }
        Ok(frames)
    }

    /// Whether the stream is inside an event, so ending now would cut one short.
    pub fn is_mid_frame(&self) -> bool {
        !self.data.is_empty() || !self.line.is_empty()
    }

    fn end_line(&mut self, frames: &mut Vec<SseFrame>) -> Result<(), SseError> {
        let line = std::mem::take(&mut self.line);
        if line.is_empty() {
            if !self.data.is_empty() {
                let joined = self.data.join(&b'\n');
                self.data.clear();
                let data: Value = serde_json::from_slice(&joined).map_err(|_| SseError::NotJson)?;
                let Value::Object(data) = data else {
                    return Err(SseError::NotObject);
                };
                frames.push(SseFrame {
                    id: self.id.clone(),
                    data,
                });
            }
            return Ok(());
        }
        if line.first() == Some(&b':') {
            return Ok(());
        }
        let (field, value) = match line.iter().position(|b| *b == b':') {
            Some(at) => {
                let value = &line[at + 1..];
                (&line[..at], value.strip_prefix(b" ").unwrap_or(value).to_vec())
            }
            None => (line.as_slice(), Vec::new()),
        };
        match field {
            b"data" => self.data.push(value),
            b"id" if !value.contains(&0) => {
                self.id = Some(String::from_utf8_lossy(&value).into_owned());
            }
            _ => {}
        }
        Ok(())
    }
}
