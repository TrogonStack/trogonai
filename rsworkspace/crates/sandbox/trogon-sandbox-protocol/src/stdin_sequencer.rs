use std::collections::BTreeMap;

use trogon_sandbox_contract::{StdinChunk, StdinEof, StdinSeq};

use crate::{StdinAdmit, StdinWrite};

/// Ordering and idempotency for interactive stdin writes, shared by every worker.
///
/// A client numbers its writes with [`StdinSeq`] and retries a write with the same number when the transport
/// fails. The sequencer turns that into at-most-once, in-order delivery to the process: it admits a write, tells the
/// worker which bytes to apply, and only advances once the worker confirms the bytes reached the process, so a
/// failed write can be retried unchanged.
#[derive(Debug, Default)]
pub struct StdinSequencer {
    next: StdinSeq,
    closed: bool,
    held: BTreeMap<StdinSeq, StdinWrite>,
    held_bytes: usize,
    pending: Vec<StdinWrite>,
}

impl StdinSequencer {
    /// How many sequence numbers ahead of the next expected one are held for reordering.
    pub const REORDER_WINDOW_CHUNKS: u64 = 32;

    /// How many bytes of out-of-order writes are held for reordering.
    pub const REORDER_WINDOW_BYTES: usize = 1024 * 1024;

    /// A sequencer expecting the write numbered 0 with stdin open.
    pub fn new() -> Self {
        Self::default()
    }

    /// The next write number the sequencer will apply.
    pub fn next_seq(&self) -> StdinSeq {
        self.next
    }

    /// Whether an applied write closed stdin.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Decides what to do with the write numbered `seq`.
    pub fn admit(&mut self, seq: StdinSeq, data: StdinChunk, eof: StdinEof) -> StdinAdmit {
        if seq < self.next {
            return StdinAdmit::Duplicate;
        }
        if self.closed {
            return StdinAdmit::Closed;
        }
        let write = StdinWrite { data, eof };
        if seq == self.next {
            let run = self.run_from(write);
            self.pending = run.clone();
            return StdinAdmit::Apply(run);
        }
        if seq.get() - self.next.get() > Self::REORDER_WINDOW_CHUNKS {
            return StdinAdmit::OutOfWindow;
        }
        if self.held.contains_key(&seq) {
            return StdinAdmit::Buffered;
        }
        if self.held_bytes + write.data.as_slice().len() > Self::REORDER_WINDOW_BYTES {
            return StdinAdmit::OutOfWindow;
        }
        self.held_bytes += write.data.as_slice().len();
        self.held.insert(seq, write);
        StdinAdmit::Buffered
    }

    /// Records that the first `applied` writes of the last [`StdinAdmit::Apply`] reached the process.
    pub fn commit(&mut self, applied: usize) {
        let run = std::mem::take(&mut self.pending);
        for write in run.into_iter().take(applied) {
            self.held.remove(&self.next);
            self.next = Self::following(self.next);
            if write.eof.closes() {
                self.closed = true;
                break;
            }
        }
        let next = self.next;
        self.held.retain(|held_seq, _| *held_seq >= next);
        if self.closed {
            self.held.clear();
        }
        self.held_bytes = self.held.values().map(|write| write.data.as_slice().len()).sum();
    }

    fn following(seq: StdinSeq) -> StdinSeq {
        StdinSeq::new(seq.get().saturating_add(1))
    }

    fn run_from(&self, first: StdinWrite) -> Vec<StdinWrite> {
        let mut run = Vec::new();
        let mut seq = self.next;
        let mut write = first;
        loop {
            let closes = write.eof.closes();
            run.push(write);
            if closes {
                break;
            }
            seq = Self::following(seq);
            match self.held.get(&seq) {
                Some(next) => write = next.clone(),
                None => break,
            }
        }
        run
    }
}
