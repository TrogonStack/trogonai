use std::collections::VecDeque;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use trogon_sandbox_contract::TerminalOutput;

use super::ReadOutcome;
use crate::constants::MAX_BUFFERED_BYTES;

#[derive(Default)]
pub(super) struct Buffer {
    bytes: VecDeque<u8>,
    /// Bytes dropped because the buffer was full, since the last read.
    dropped_bytes: u64,
    /// Frames the subscription never delivered, since the last read.
    missed_frames: u64,
    next_seq: u64,
    /// Set once the terminal ended: `Some(code)` from its `Exit` frame, or `Some(None)` when the output stream
    /// closed first.
    ended: Option<Option<i32>>,
}

impl Buffer {
    pub(super) fn push(&mut self, frame: TerminalOutput) {
        match frame {
            // A resync only answers a reattach, which this buffer never sends.
            TerminalOutput::Resync { .. } => {}
            TerminalOutput::Exit { seq, exit_code } => {
                self.advance(seq);
                self.ended = Some(exit_code);
            }
            TerminalOutput::Data { seq, data } => {
                self.advance(seq);
                let bytes = STANDARD.decode(data.as_str()).unwrap_or_default();
                self.bytes.extend(bytes);
                let over = self.bytes.len().saturating_sub(MAX_BUFFERED_BYTES);
                self.bytes.drain(..over);
                self.dropped_bytes += over as u64;
            }
        }
    }

    /// Marks the output stream as closed without an `Exit` frame.
    pub(super) fn close(&mut self) {
        self.ended.get_or_insert(None);
    }

    fn advance(&mut self, seq: u64) {
        self.missed_frames += seq.saturating_sub(self.next_seq);
        self.next_seq = self.next_seq.max(seq + 1);
    }

    pub(super) fn ready(&self) -> bool {
        !self.bytes.is_empty() || self.ended.is_some()
    }

    pub(super) fn take(&mut self, max_bytes: usize) -> ReadOutcome {
        let mut take = self.bytes.len().min(max_bytes);
        let head: Vec<u8> = self.bytes.iter().take(take).copied().collect();
        // Hold back a character split by the limit or by a frame boundary, unless nothing more can arrive to
        // complete it.
        if let Err(e) = std::str::from_utf8(&head)
            && e.error_len().is_none()
            && self.ended.is_none()
            && e.valid_up_to() > 0
        {
            take = e.valid_up_to();
        }
        let output: Vec<u8> = self.bytes.drain(..take).collect();
        let exited = self.bytes.is_empty() && self.ended.is_some();
        ReadOutcome {
            output: String::from_utf8_lossy(&output).into_owned(),
            dropped_bytes: std::mem::take(&mut self.dropped_bytes),
            missed_frames: std::mem::take(&mut self.missed_frames),
            exited,
            exit_code: if exited { self.ended.flatten() } else { None },
        }
    }
}

#[cfg(test)]
mod tests;
