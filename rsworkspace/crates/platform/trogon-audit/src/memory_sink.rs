use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;

use crate::audit_sink::AuditSink;
use crate::checkpoint_sink::CheckpointSink;
use crate::constants::{STREAM_AUDIT, STREAM_AUDIT_CHECKPOINT, STREAM_DUPLICATE_WINDOW};
use crate::sealed_record::SealedRecord;
use crate::signed_checkpoint::SignedCheckpoint;
use crate::sink_error::SinkError;
use crate::stored_ack::StoredAck;
use crate::stored_audit_record::StoredAuditRecord;

/// An in-memory stream with failure injection, for testing how a surface behaves
/// when audit is down, full or slow to acknowledge.
///
/// Like the `AUDIT` stream it stands in for, it numbers what it stores from 1 and
/// remembers each record id for a duplicate window: a publish under an id it
/// stored inside the window is acknowledged as a duplicate at the first copy's
/// sequence and stores nothing, and one after the window is stored again. The
/// window is measured on the tokio clock, so a paused test can step past it.
///
/// It also stands in for `AUDIT_CHECKPOINT`: checkpoints share its attempts,
/// holds and failures, and are kept apart from the records.
#[derive(Debug, Clone)]
pub struct MemorySink {
    state: Arc<Mutex<MemoryState>>,
    held: Arc<watch::Sender<bool>>,
}

#[derive(Debug)]
struct MemoryState {
    stored: Vec<StoredMessage>,
    checkpoints: Vec<SignedCheckpoint>,
    attempts: usize,
    failures: VecDeque<SinkError>,
    lost_acks: usize,
    failing: Option<SinkError>,
    duplicate_window: Duration,
}

#[derive(Debug)]
struct StoredMessage {
    record: SealedRecord,
    stream_sequence: u64,
    stored_at: Instant,
}

impl Default for MemorySink {
    fn default() -> Self {
        Self::new()
    }
}

impl MemorySink {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(MemoryState {
                stored: Vec::new(),
                checkpoints: Vec::new(),
                attempts: 0,
                failures: VecDeque::new(),
                lost_acks: 0,
                failing: None,
                duplicate_window: STREAM_DUPLICATE_WINDOW,
            })),
            held: Arc::new(watch::Sender::new(false)),
        }
    }

    /// How long a record id is remembered. The default is the `AUDIT` stream's.
    pub fn with_duplicate_window(self, window: Duration) -> Self {
        self.state().duplicate_window = window;
        self
    }

    fn state(&self) -> MutexGuard<'_, MemoryState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every message stored, in stream order. A record stored again after the
    /// duplicate window appears once per copy.
    pub fn stored(&self) -> Vec<SealedRecord> {
        self.state()
            .stored
            .iter()
            .map(|message| message.record.clone())
            .collect()
    }

    /// The stored messages as a reader of the stream sees them: the stream
    /// sequence, the payload, the subject and the headers, ready for a verifier.
    pub fn stored_records(&self) -> Vec<StoredAuditRecord> {
        self.state()
            .stored
            .iter()
            .map(|message| {
                StoredAuditRecord::new(message.stream_sequence, message.record.payload().clone())
                    .with_subject(message.record.subject())
                    .with_headers(&message.record.headers())
            })
            .collect()
    }

    /// Every checkpoint stored, in stream order.
    pub fn checkpoints(&self) -> Vec<SignedCheckpoint> {
        self.state().checkpoints.clone()
    }

    /// How many publishes were attempted, stored or not, counted as each begins.
    pub fn attempts(&self) -> usize {
        self.state().attempts
    }

    /// The next `count` publishes fail with `error` and store nothing.
    pub fn fail_next(&self, count: usize, error: SinkError) {
        self.state().failures.extend(std::iter::repeat_n(error, count));
    }

    /// Every publish fails with `error` until [`MemorySink::recover`].
    pub fn fail_all(&self, error: SinkError) {
        self.state().failing = Some(error);
    }

    /// The next `count` publishes store the record but report a timeout, as a
    /// lost acknowledgement does.
    pub fn lose_next_acks(&self, count: usize) {
        self.state().lost_acks = count;
    }

    /// Publishes begin, and count as attempts, but wait without storing until
    /// [`MemorySink::release`], as a publish waiting on a slow stream does.
    pub fn hold(&self) {
        self.held.send_replace(true);
    }

    pub fn release(&self) {
        self.held.send_replace(false);
    }

    pub fn recover(&self) {
        let mut state = self.state();
        state.failing = None;
        state.failures.clear();
        state.lost_acks = 0;
    }
}

impl MemorySink {
    /// Counts the attempt, waits while held, then takes the injected failure if
    /// one is due.
    async fn begin(&self) -> Result<MutexGuard<'_, MemoryState>, SinkError> {
        self.state().attempts += 1;
        let mut held = self.held.subscribe();
        drop(held.wait_for(|held| !*held).await);
        let mut state = self.state();
        if let Some(error) = state.failing {
            return Err(error);
        }
        if let Some(error) = state.failures.pop_front() {
            return Err(error);
        }
        Ok(state)
    }
}

impl AuditSink for MemorySink {
    async fn publish(&self, record: &SealedRecord) -> Result<StoredAck, SinkError> {
        let mut state = self.begin().await?;
        let now = Instant::now();
        let window = state.duplicate_window;
        let original = state
            .stored
            .iter()
            .find(|message| message.record.id() == record.id() && now.duration_since(message.stored_at) < window)
            .map(|message| message.stream_sequence);
        let ack = match original {
            Some(stream_sequence) => StoredAck::new(STREAM_AUDIT, stream_sequence, true),
            None => {
                let stream_sequence = state.stored.len() as u64 + 1;
                state.stored.push(StoredMessage {
                    record: record.clone(),
                    stream_sequence,
                    stored_at: now,
                });
                StoredAck::new(STREAM_AUDIT, stream_sequence, false)
            }
        };
        if state.lost_acks > 0 {
            state.lost_acks -= 1;
            return Err(SinkError::Timeout);
        }
        Ok(ack)
    }
}

impl CheckpointSink for MemorySink {
    async fn publish_checkpoint(&self, checkpoint: &SignedCheckpoint) -> Result<StoredAck, SinkError> {
        let mut state = self.begin().await?;
        state.checkpoints.push(checkpoint.clone());
        Ok(StoredAck::new(
            STREAM_AUDIT_CHECKPOINT,
            state.checkpoints.len() as u64,
            false,
        ))
    }
}

#[cfg(test)]
mod tests;
