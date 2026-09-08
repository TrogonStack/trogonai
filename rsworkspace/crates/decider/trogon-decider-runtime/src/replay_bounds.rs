use crate::{ReadAfterOverflowError, ReadFrom, ReplayChunkSize, ReplayLimit, StreamEvent, StreamPosition};

/// How much history one command execution may replay, and how much of it may be
/// resident at once.
///
/// The two halves answer different questions and a host can set either alone.
/// [`ReplayLimit`] is a correctness tripwire: past it the command fails rather
/// than replaying a stream that has outgrown its snapshot cadence.
/// [`ReplayChunkSize`] is a memory bound: it says nothing about how much history
/// is acceptable, only how much of it may be held at one time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReplayBounds {
    limit: Option<ReplayLimit>,
    chunk_size: Option<ReplayChunkSize>,
}

impl ReplayBounds {
    /// Bounds an execution by the limit and chunk size a host configured, either
    /// of which may be absent.
    pub const fn new(limit: Option<ReplayLimit>, chunk_size: Option<ReplayChunkSize>) -> Self {
        Self { limit, chunk_size }
    }

    /// The configured replay limit, if any.
    pub const fn limit(self) -> Option<ReplayLimit> {
        self.limit
    }

    /// The configured chunk size, if any.
    pub const fn chunk_size(self) -> Option<ReplayChunkSize> {
        self.chunk_size
    }

    /// How many events the next read may fetch, given how many this execution
    /// has already replayed. `None` reads whatever is left of the stream.
    ///
    /// A limit contributes one more than its remaining allowance, because a read
    /// that comes back with that extra event proves the limit was exceeded
    /// without fetching the rest of the stream to find out by how much.
    pub fn read_bound(self, replayed_event_count: u64) -> Option<u64> {
        let limit_bound = self
            .limit
            .map(|limit| limit.as_u64().saturating_sub(replayed_event_count).saturating_add(1));
        let chunk_bound = self.chunk_size.map(ReplayChunkSize::as_u64);
        match (limit_bound, chunk_bound) {
            (Some(limit_bound), Some(chunk_bound)) => Some(limit_bound.min(chunk_bound)),
            (bound @ Some(_), None) | (None, bound @ Some(_)) => bound,
            (None, None) => None,
        }
    }
}

/// Walks a stream's history in [`ReplayBounds`]-sized chunks, pinned to the tail
/// the first read observed.
///
/// The pin is what keeps a chunked replay honest. Reads happen one after
/// another, so the stream can grow between them; folding whatever the later
/// reads happen to return would build state from history the append is not
/// guarded against, and the optimistic-concurrency precondition would then be
/// asserting a position the decision never saw. Every chunk is therefore cut off
/// at the position the first read reported, and events past it are left for the
/// next execution, exactly as an unchunked replay leaves them.
#[derive(Debug, Clone, Copy)]
pub struct ReplayCursor {
    bounds: ReplayBounds,
    tail: Option<StreamPosition>,
    last_replayed: Option<StreamPosition>,
    replayed_event_count: u64,
}

impl ReplayCursor {
    /// Starts a cursor over a stream whose high-watermark the first read
    /// observed as `tail`.
    pub const fn new(bounds: ReplayBounds, tail: Option<StreamPosition>) -> Self {
        Self {
            bounds,
            tail,
            last_replayed: None,
            replayed_event_count: 0,
        }
    }

    /// Drops the events a read returned from past the pinned tail.
    ///
    /// A read that reported no tail at all is left alone. Events without a
    /// high-watermark to place them under is a store contradicting itself, and
    /// silently dropping them here would turn that into a wrong answer instead
    /// of whatever the fold makes of it.
    pub fn truncate_to_tail(&self, events: &mut Vec<StreamEvent>) {
        let Some(tail) = self.tail else {
            return;
        };
        if let Some(past_tail) = events.iter().position(|event| event.stream_position > tail) {
            events.truncate(past_tail);
        }
    }

    /// Records a chunk as replayed and returns the running total.
    pub fn advance(&mut self, chunk: &[StreamEvent]) -> u64 {
        if let Some(last) = chunk.last() {
            self.last_replayed = Some(last.stream_position);
        }
        self.replayed_event_count = self.replayed_event_count.saturating_add(chunk.len() as u64);
        self.replayed_event_count
    }

    /// How many events have been replayed so far.
    pub const fn replayed_event_count(self) -> u64 {
        self.replayed_event_count
    }

    /// How many events the next read may fetch, or `None` for whatever is left.
    pub fn read_bound(self) -> Option<u64> {
        self.bounds.read_bound(self.replayed_event_count)
    }

    /// Where the next chunk starts, or `None` when the pinned tail is reached.
    ///
    /// A read that came back empty ends the walk too: without a chunk to advance
    /// past, asking again would fetch the same nothing forever.
    pub fn next_read(self) -> Option<Result<ReadFrom, ReadAfterOverflowError>> {
        self.bounds.chunk_size?;
        let last_replayed = self.last_replayed?;
        if self.tail.is_none_or(|tail| last_replayed >= tail) {
            return None;
        }
        Some(ReadFrom::after(last_replayed))
    }
}

#[cfg(test)]
mod tests;
