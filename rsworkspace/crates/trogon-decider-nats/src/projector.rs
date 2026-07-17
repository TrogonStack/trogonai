//! Generic catch-up driver for subject-filtered JetStream projections.
//!
//! [`Projector`] owns the parts of a read-model catch-up that stay identical
//! across backends: creating the subject-filtered ordered consumer (via
//! [`crate::stream_store`]'s replay helpers), finding the stream's current
//! tail, and applying events in order. What differs across backends is where
//! the checkpoint lives and how it advances, which [`ProjectionCheckpointStore`]
//! captures behind one contract:
//!
//! - A NATS KV backed checkpoint has no transaction to piggyback on, so the
//!   driver calls [`ProjectionCheckpointStore::save`] after every applied
//!   event and that call does the real CAS write.
//! - A Postgres-backed (or any externally transactional) checkpoint advances
//!   atomically with the projection write inside
//!   [`ProjectionApply::apply`]: the closure commits both in one transaction
//!   and returns the checkpoint it just committed, and the corresponding
//!   `save` implementation is a no-op because the write already happened.
//!
//! Both shapes fit the same loop because the apply step, not the driver,
//! decides what checkpoint value to record next.
//!
//! [`Projector::catch_up`] resumes from whatever [`ProjectionCheckpointStore::load`]
//! returns. A caller that wants to always reconcile from scratch (ignoring any
//! stored checkpoint) can compose that by implementing `load` to always return
//! [`CheckpointSequence::NONE`]; the driver itself has no such policy baked in.
//!
//! The stream tail used as the catch-up target is captured once, at the start
//! of the call, via a fresh [`jetstream::stream::Stream::get_info`]. Events
//! published after that point are not included; call [`Projector::catch_up`]
//! again to pick them up.

use async_nats::jetstream;
use trogon_decider_runtime::StreamEvent;

use crate::stream_store::{ReadStreamError, StreamStoreError, read_stream_range, read_subject_stream};

/// A projection checkpoint position.
///
/// `CheckpointSequence` wraps the JetStream stream sequence up to and
/// including which a projection has been applied. [`CheckpointSequence::NONE`]
/// means no event has been applied yet, distinct from sequence `1` (the first
/// possible event).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CheckpointSequence(u64);

impl CheckpointSequence {
    /// No projection progress has been recorded yet.
    pub const NONE: Self = Self(0);

    /// Wraps an already known checkpoint sequence.
    pub const fn new(sequence: u64) -> Self {
        Self(sequence)
    }

    /// Returns the checkpoint as a plain stream sequence.
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Returns whether no projection progress has been recorded yet.
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }

    /// Returns the stream sequence a catch-up should resume from.
    ///
    /// This is the checkpoint's sequence plus one: [`CheckpointSequence::NONE`]
    /// resumes from sequence `1`, the first possible event.
    pub const fn next_from_sequence(self) -> u64 {
        self.0.saturating_add(1)
    }
}

impl From<trogon_decider_runtime::StreamPosition> for CheckpointSequence {
    fn from(position: trogon_decider_runtime::StreamPosition) -> Self {
        Self(position.as_u64())
    }
}

/// Loads and saves a projection's checkpoint.
///
/// See the module documentation for how this contract fits both a
/// NATS KV owned checkpoint and an externally transactional one.
pub trait ProjectionCheckpointStore: Send + Sync {
    /// Error raised while loading or saving the checkpoint.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Loads the checkpoint to resume a catch-up from.
    fn load(&self) -> impl std::future::Future<Output = Result<CheckpointSequence, Self::Error>> + Send;

    /// Records a new checkpoint after an event has been applied.
    ///
    /// Implementations backed by an externally transactional checkpoint (for
    /// example, one advanced inside the same Postgres transaction as the
    /// projection write) should make this a no-op: the checkpoint was already
    /// committed by [`ProjectionApply::apply`].
    fn save(&self, checkpoint: CheckpointSequence)
    -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;
}

/// Applies one projected event and returns the checkpoint to record for it.
///
/// The returned [`CheckpointSequence`] is what [`Projector`] passes to
/// [`ProjectionCheckpointStore::save`] immediately afterward. Implementations
/// that commit their own checkpoint transactionally alongside the projection
/// write should still return the sequence they committed, even though the
/// paired `save` call will be a no-op.
pub trait ProjectionApply: Send {
    /// Error raised while applying a projected event.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Applies one event, returning the checkpoint to record for it.
    fn apply(
        &mut self,
        event: StreamEvent,
    ) -> impl std::future::Future<Output = Result<CheckpointSequence, Self::Error>> + Send;
}

/// Error raised by [`Projector::catch_up`].
#[derive(Debug, thiserror::Error)]
pub enum CatchUpError<CheckpointError, ApplyError> {
    /// Loading the checkpoint to resume from failed.
    #[error("failed to load projection checkpoint: {0}")]
    LoadCheckpoint(#[source] CheckpointError),
    /// Querying the stream's current tail failed.
    #[error("failed to query stream tail: {0}")]
    QueryTail(#[source] StreamStoreError),
    /// Replaying stream events failed.
    #[error("failed to replay stream events: {0}")]
    Replay(#[source] StreamStoreError),
    /// Applying a projected event failed.
    #[error("failed to apply projected event at sequence {sequence}: {source}")]
    Apply {
        /// Stream sequence of the event that failed to apply.
        sequence: u64,
        /// Underlying application error.
        #[source]
        source: ApplyError,
    },
    /// Saving the checkpoint after applying an event failed.
    #[error("failed to save projection checkpoint: {0}")]
    SaveCheckpoint(#[source] CheckpointError),
}

/// Result of one [`Projector::catch_up`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatchUpOutcome {
    /// Number of events applied during this call.
    pub events_applied: usize,
    /// Checkpoint recorded after the call completed.
    pub checkpoint: CheckpointSequence,
    /// Whether the replay reached the stream tail observed at the start of the call.
    ///
    /// This is `true` on every successful return: a stream ending before the
    /// observed tail is a transient condition retried internally, and only
    /// gives up (returning [`CatchUpError::Replay`]) after exhausting
    /// retries. The field stays meaningful as a caller-facing signal for
    /// "did this call make full progress", separate from how that guarantee
    /// is implemented.
    pub reached_target: bool,
}

/// Drives catch-up for one subject-filtered projection over a JetStream stream.
pub struct Projector<Checkpoint> {
    stream: jetstream::stream::Stream,
    projection_id: String,
    filter_subject: Option<String>,
    checkpoint: Checkpoint,
}

impl<Checkpoint> Projector<Checkpoint>
where
    Checkpoint: ProjectionCheckpointStore,
{
    /// Creates a projector over the full stream, applying every event regardless of subject.
    pub fn new(stream: jetstream::stream::Stream, projection_id: impl Into<String>, checkpoint: Checkpoint) -> Self {
        Self {
            stream,
            projection_id: projection_id.into(),
            filter_subject: None,
            checkpoint,
        }
    }

    /// Restricts catch-up to events published on `filter_subject`.
    #[must_use]
    pub fn with_filter_subject(mut self, filter_subject: impl Into<String>) -> Self {
        self.filter_subject = Some(filter_subject.into());
        self
    }

    /// Returns the checkpoint store backing this projector.
    pub fn checkpoint_store(&self) -> &Checkpoint {
        &self.checkpoint
    }

    /// Catches a projection up to the stream tail observed when this call started.
    ///
    /// Resumes from whatever [`ProjectionCheckpointStore::load`] returns.
    /// Applies events in stream order, saving the checkpoint after each one.
    pub async fn catch_up<Apply>(
        &self,
        mut apply: Apply,
    ) -> Result<CatchUpOutcome, CatchUpError<Checkpoint::Error, Apply::Error>>
    where
        Apply: ProjectionApply,
    {
        let checkpoint = self.checkpoint.load().await.map_err(CatchUpError::LoadCheckpoint)?;
        let from_sequence = checkpoint.next_from_sequence();

        let info = self
            .stream
            .get_info()
            .await
            .map_err(|source| CatchUpError::QueryTail(ReadStreamError::QueryStreamInfo { source }.into()))?;
        let to_sequence = info.state.last_sequence;

        let events = self
            .replay(from_sequence, to_sequence)
            .await
            .map_err(CatchUpError::Replay)?;

        let events_applied = events.len();
        let mut checkpoint = checkpoint;
        for event in events {
            let sequence = event.stream_position.as_u64();
            checkpoint = apply
                .apply(event)
                .await
                .map_err(|source| CatchUpError::Apply { sequence, source })?;
            self.checkpoint
                .save(checkpoint)
                .await
                .map_err(CatchUpError::SaveCheckpoint)?;
        }

        Ok(CatchUpOutcome {
            events_applied,
            checkpoint,
            reached_target: true,
        })
    }

    async fn replay(&self, from_sequence: u64, to_sequence: u64) -> Result<Vec<StreamEvent>, StreamStoreError> {
        match self.filter_subject.as_deref() {
            Some(subject) => {
                read_subject_stream(
                    &self.stream,
                    self.projection_id.as_str(),
                    subject,
                    from_sequence,
                    to_sequence,
                )
                .await
            }
            None => read_stream_range(&self.stream, from_sequence, to_sequence).await,
        }
    }
}

#[cfg(test)]
mod tests;
