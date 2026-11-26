use std::collections::VecDeque;

use trogon_sandbox_contract::OutputTruncation;

use crate::output_cap::join;

/// Accumulates one stream, retaining at most `cap` bytes of it.
///
/// A command can print without limit. It keeps the first and last halves of the budget and drops the middle, so a
/// runaway command costs bounded memory while its start (usually the invocation context) and its end (usually the
/// failure) both survive.
#[derive(Debug)]
pub struct CappedOutput {
    head_cap: usize,
    tail_cap: usize,
    head: Vec<u8>,
    tail: VecDeque<u8>,
    dropped: u64,
}

impl CappedOutput {
    pub fn new(cap: usize) -> Self {
        let head_cap = cap / 2;
        Self {
            head_cap,
            tail_cap: cap - head_cap,
            head: Vec::new(),
            tail: VecDeque::new(),
            dropped: 0,
        }
    }

    pub fn push(&mut self, data: &[u8]) {
        let head_room = self.head_cap - self.head.len();
        let (to_head, rest) = data.split_at(head_room.min(data.len()));
        self.head.extend_from_slice(to_head);
        if rest.is_empty() {
            return;
        }
        // Only the last `tail_cap` bytes of `rest` can survive.
        let skipped = rest.len().saturating_sub(self.tail_cap);
        self.dropped += skipped as u64;
        self.tail.extend(&rest[skipped..]);
        let overflow = self.tail.len().saturating_sub(self.tail_cap);
        if overflow > 0 {
            self.tail.drain(..overflow);
            self.dropped += overflow as u64;
        }
    }

    /// Records that the caller already dropped `bytes` between what was pushed so far and what is pushed next.
    ///
    /// The head closes, so later data lands in the tail.
    pub fn skip(&mut self, bytes: u64) {
        if bytes == 0 {
            return;
        }
        self.head_cap = self.head.len();
        self.dropped += bytes;
    }

    /// Whether anything was dropped.
    pub fn truncation(&self) -> OutputTruncation {
        OutputTruncation::from(self.dropped > 0)
    }

    /// The retained bytes, with a one-line marker in place of the dropped middle when anything was dropped.
    pub fn into_bytes(self) -> Vec<u8> {
        let tail: Vec<u8> = self.tail.into();
        join(self.head, self.dropped, tail)
    }
}
