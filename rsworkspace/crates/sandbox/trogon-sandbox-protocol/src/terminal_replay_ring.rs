use std::collections::VecDeque;

use trogon_sandbox_contract::{Base64DataError, TerminalOutput};

use crate::terminal_data_frame;

/// The latest output of one terminal, bounded in bytes, for a reader that reattaches.
///
/// A worker keeps a small in-memory ring of the latest output and, when the caller opens an id that is already
/// open, republishes those frames before live output resumes. The ring is memory only, dies with the terminal,
/// and is off when sized to zero. It is fed by the terminal's single writer with the same `seq` it publishes, so
/// replayed frames keep their original `seq` and a reader that already saw them drops anything at or below its
/// high-water mark. The oldest bytes are dropped first, and a frame larger than the whole ring keeps only its
/// tail.
#[derive(Debug, Clone)]
pub struct TerminalReplayRing {
    capacity: usize,
    bytes: usize,
    frames: VecDeque<(u64, Vec<u8>)>,
    /// Newest `seq` recorded, kept even when nothing is retained.
    newest: Option<u64>,
    /// Highest `seq` of which any byte was dropped or trimmed. A reader that has seen at least this much misses
    /// nothing the ring forgot.
    lost_through: Option<u64>,
}

impl TerminalReplayRing {
    /// Default ring size: enough for a screenful or two, small enough to hold for every open terminal.
    pub const DEFAULT_CAPACITY: usize = 8 * 1024;

    /// Largest ring a worker accepts from configuration.
    pub const MAX_CAPACITY: usize = 1024 * 1024;

    /// A ring holding at most `capacity` bytes, clamped to [`MAX_CAPACITY`](Self::MAX_CAPACITY). Zero keeps
    /// nothing.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.min(Self::MAX_CAPACITY),
            bytes: 0,
            frames: VecDeque::new(),
            newest: None,
            lost_through: None,
        }
    }

    /// A ring of [`DEFAULT_CAPACITY`](Self::DEFAULT_CAPACITY) bytes.
    pub fn with_default_capacity() -> Self {
        Self::new(Self::DEFAULT_CAPACITY)
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Bytes currently held.
    pub fn len(&self) -> usize {
        self.bytes
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Records the `data` frame published as `seq`.
    pub fn push(&mut self, seq: u64, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.newest = Some(self.newest.map_or(seq, |newest| newest.max(seq)));
        if self.capacity == 0 {
            self.lost_through = Some(seq);
            return;
        }
        let cut = data.len().saturating_sub(self.capacity);
        if cut > 0 {
            self.lost_through = Some(seq);
        }
        let kept = &data[cut..];
        self.bytes += kept.len();
        self.frames.push_back((seq, kept.to_vec()));
        while self.bytes > self.capacity {
            match self.frames.pop_front() {
                Some((dropped_seq, dropped)) => {
                    self.bytes -= dropped.len();
                    self.lost_through = Some(dropped_seq);
                }
                None => break,
            }
        }
    }

    /// The frames to send a reader that reattaches having seen output up to `resume_from`.
    ///
    /// `None` means it has seen nothing, or does not say, so it gets everything retained. When bytes after
    /// `resume_from` were forgotten the answer starts with a `resync` frame, followed by everything retained, so
    /// the reader can reset and carry on.
    pub fn reattach_frames(&self, resume_from: Option<u64>) -> Result<Vec<TerminalOutput>, Base64DataError> {
        let Some(seen) = resume_from else {
            return self.replay_frames();
        };
        if self.lost_through.is_some_and(|lost| lost > seen) {
            let from_seq = self
                .frames
                .front()
                .map(|(seq, _)| *seq)
                .or_else(|| self.newest.map(|newest| newest.saturating_add(1)))
                .unwrap_or(0);
            let mut out = vec![TerminalOutput::Resync { from_seq }];
            out.extend(self.replay_frames()?);
            return Ok(out);
        }
        self.frames
            .iter()
            .filter(|(seq, _)| *seq > seen)
            .map(|(seq, bytes)| terminal_data_frame(*seq, bytes))
            .collect()
    }

    /// The retained frames, oldest first, as the `data` frames to republish to a reattaching reader. Each keeps the
    /// `seq` it was first sent with.
    pub fn replay_frames(&self) -> Result<Vec<TerminalOutput>, Base64DataError> {
        self.frames
            .iter()
            .map(|(seq, bytes)| terminal_data_frame(*seq, bytes))
            .collect()
    }
}
