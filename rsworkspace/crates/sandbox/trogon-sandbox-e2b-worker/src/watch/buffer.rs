use std::collections::VecDeque;

use tokio::time::Instant;
use trogon_sandbox_contract::{FileOp, FileOpResponse, FileWatchEvent, FileWatchOverflow, FileWatchTarget};

use super::constants::WATCH_BUFFER_EVENTS;
use super::event_path::event_path;
use crate::envd::WatcherEvent;

/// The sequence-numbered events of one watch.
pub(super) struct Buffer {
    pub(super) events: VecDeque<FileWatchEvent>,
    next_seq: u64,
    pub(super) dropped_through: u64,
    /// `dropped_through` as of the last evidence publish.
    pub(super) reported_through: u64,
    pub(super) touched: Instant,
}

impl Buffer {
    pub(super) fn new() -> Self {
        Self {
            events: VecDeque::new(),
            next_seq: 1,
            dropped_through: 0,
            reported_through: 0,
            touched: Instant::now(),
        }
    }

    pub(super) fn push(&mut self, root: &str, events: Vec<WatcherEvent>, lost: usize) {
        for event in events {
            self.append(FileWatchEvent {
                seq: 0,
                kind: event.kind,
                path: event_path(root, &event.name),
                is_dir: FileWatchTarget::from(event.is_dir),
            });
        }
        // Events envd returned past the per-fetch ceiling never got a sequence number; the gap is reported the same
        // way as a buffer overflow.
        if lost > 0 {
            self.dropped_through = self.next_seq.saturating_sub(1).max(self.dropped_through);
        }
    }

    fn append(&mut self, mut event: FileWatchEvent) {
        event.seq = self.next_seq;
        self.next_seq += 1;
        self.events.push_back(event);
        if self.events.len() > WATCH_BUFFER_EVENTS
            && let Some(dropped) = self.events.pop_front()
        {
            self.dropped_through = dropped.seq;
        }
    }

    /// The events past `after_seq`, or `None` when there are none.
    pub(super) fn ready(&self, after_seq: u64) -> Option<FileOpResponse> {
        let events: Vec<FileWatchEvent> = self
            .events
            .iter()
            .filter(|event| event.seq > after_seq)
            .take(FileOp::WATCH_MAX_EVENTS_PER_POLL)
            .cloned()
            .collect();
        let next_seq = events.last()?.seq;
        Some(FileOpResponse::WatchEvents {
            events,
            next_seq,
            overflowed: self.overflowed_past(after_seq),
        })
    }

    /// Whether events fell out of the buffer after `after_seq`.
    pub(super) fn overflowed_past(&self, after_seq: u64) -> FileWatchOverflow {
        FileWatchOverflow::from(self.dropped_through > after_seq)
    }
}
