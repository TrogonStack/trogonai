//! The server-sent events wire parser of the browser transport.

use crate::{sse_frame::SseFrame, sse_utf8_decoder::SseUtf8Decoder};

/// Splits an SSE byte stream into frames. Lines end at LF, CRLF or CR and a blank line dispatches the frame, so a
/// chunk may end anywhere, including between the CR and LF of a line ending.
#[derive(Debug)]
pub struct SseParser {
    decoder: SseUtf8Decoder,
    pending: String,
    dispatched: u64,
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SseParser {
    /// A parser with nothing buffered.
    pub fn new() -> Self {
        Self {
            decoder: SseUtf8Decoder::new(),
            pending: String::new(),
            dispatched: 0,
        }
    }

    /// Feeds a chunk of the byte stream and returns the frames it completes.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseFrame> {
        let text = self.decoder.decode(bytes, true);
        self.push_str(&text)
    }

    /// Feeds already decoded text and returns the frames it completes.
    pub fn push_str(&mut self, text: &str) -> Vec<SseFrame> {
        if text.is_empty() {
            return Vec::new();
        }
        // A delimiter that straddles two chunks starts at most three bytes before the end of what is buffered.
        let mut from = self.pending.len().saturating_sub(3);
        self.pending.push_str(text);
        let mut frames = Vec::new();
        let mut consumed = 0;
        while let Some((content_end, after)) = next_delimiter(self.pending.as_bytes(), from) {
            frames.extend(SseFrame::parse(&self.pending[consumed..content_end]));
            self.dispatched += 1;
            consumed = after;
            from = after;
        }
        self.pending.drain(..consumed);
        frames
    }

    /// Ends the stream: parses whatever is still buffered as one last frame and starts over.
    pub fn flush(&mut self) -> Vec<SseFrame> {
        let rest = self.decoder.decode(&[], false);
        self.pending.push_str(&rest);
        let block = std::mem::take(&mut self.pending);
        SseFrame::parse(&block).into_iter().collect()
    }

    /// The text received but not yet dispatched.
    pub fn buffered(&self) -> &str {
        &self.pending
    }

    /// How many UTF-16 code units are buffered.
    pub fn pending_chars(&self) -> usize {
        self.pending.encode_utf16().count()
    }

    /// How many blank-line delimiters were seen, whether or not their block held a frame.
    pub fn frames_dispatched(&self) -> u64 {
        self.dispatched
    }
}

/// The length of the line ending at `at`: 1 for LF, 2 for CRLF, 1 for a CR with more text after it, `None` for a CR
/// that ends the buffer (its partner may still arrive), 0 when no line ending starts there.
fn line_ending(bytes: &[u8], at: usize) -> Option<usize> {
    match bytes[at] {
        b'\n' => Some(1),
        b'\r' => match bytes.get(at + 1) {
            None => None,
            Some(b'\n') => Some(2),
            Some(_) => Some(1),
        },
        _ => Some(0),
    }
}

/// The first blank-line delimiter at or after `from`: where its block's content ends and where the next block
/// begins. `None` when the buffer might still be completed by more text.
fn next_delimiter(bytes: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut at = from;
    while at < bytes.len() {
        let length = line_ending(bytes, at)?;
        if length == 0 {
            at += 1;
            continue;
        }
        let second = at + length;
        if second >= bytes.len() {
            return None;
        }
        let following = line_ending(bytes, second)?;
        if following > 0 {
            return Some((at, second + following));
        }
        at = second;
    }
    None
}

#[cfg(test)]
#[path = "sse_parser_tests.rs"]
mod tests;
