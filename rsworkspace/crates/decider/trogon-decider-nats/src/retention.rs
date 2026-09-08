//! Snapshot-derived retention watermarks for decider streams.
//!
//! A logical stream's events stop being needed once every snapshot that
//! resumes from them has moved past them: both `CommandExecution` and
//! `WasmCommandExecution` resume replay from `ReadFrom::after(snapshot.position)`,
//! so no execution path reads earlier than the oldest outstanding snapshot.
//! This module turns that observation into a number an operator can act on.
//!
//! Everything here is a read-only query. Nothing in this module purges, trims,
//! or deletes anything, and no store operation computes a watermark as a side
//! effect: deciding to truncate is an operator or scheduled-job action, per
//! [ADR#0029](https://github.com/TrogonStack/trogonai/blob/main/docs/adr/0029-decider-retention-and-truncation-watermark.md).
//!
//! Watermarks are physical JetStream stream sequences, the same ones snapshots
//! record, never `Trogon-Origin-Stream-Sequence`, which is provenance metadata
//! and absent on ordinary appends.
//!
//! # Completeness is the caller's obligation
//!
//! A watermark is only as safe as the set of markers folded into it. The
//! builder cannot discover snapshot types nobody registered with it, or
//! checkpoints held outside this bucket, so a caller that omits one gets a
//! watermark that is too high, and a purge run against it deletes events some
//! reader still needs. Every snapshot type a deployment writes, and every
//! checkpoint that tracks progress over the same physical stream, has to be
//! observed before [`RetentionWatermarksBuilder::build`].
//!
//! Everything the builder is unsure about resolves to
//! [`RetentionWatermark::RetainAll`], so an incomplete-but-declared picture
//! over-retains rather than over-deletes.

use std::collections::BTreeMap;

use trogon_decider_runtime::StreamPosition;
use trogon_decider_runtime::snapshot::{Snapshot, SnapshotPayloadDecode, SnapshotType};
use trogon_nats::jetstream::{JetStreamKvEntry, JetStreamKvGet, JetStreamKvKeys};

use crate::projector::CheckpointSequence;
use crate::snapshot_store::{
    NatsSnapshotConfig, SnapshotCodecError, SnapshotDecodePayloadError, SnapshotStoreError, SnapshotTypeError,
    read_checkpoint, read_snapshot_map,
};

/// The lowest physical stream sequence a logical stream still needs.
///
/// [`RetentionWatermark::DiscardBelow`] carries the boundary: everything
/// strictly below it is safe to discard, and the boundary event itself is
/// retained even though replay resumes after it. [`RetentionWatermark::RetainAll`]
/// is the total representation of "nothing may be discarded yet", which is
/// both the answer for a stream nobody has snapshotted and the answer whenever
/// the inputs do not add up to a boundary.
///
/// The [`Ord`] implementation orders watermarks by how much they permit
/// discarding, so [`RetainAll`](RetentionWatermark::RetainAll) is the least
/// element and [`Ord::min`] is the conservative combination of two
/// constraints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RetentionWatermark {
    /// No event may be discarded.
    RetainAll,
    /// Every sequence strictly below this position may be discarded.
    DiscardBelow(StreamPosition),
}

impl RetentionWatermark {
    /// Returns the boundary sequence, or `None` when nothing may be discarded.
    pub const fn lowest_retained_sequence(self) -> Option<StreamPosition> {
        match self {
            Self::RetainAll => None,
            Self::DiscardBelow(position) => Some(position),
        }
    }

    /// Returns whether the watermark permits discarding anything at all.
    pub const fn retains_all(self) -> bool {
        matches!(self, Self::RetainAll)
    }
}

impl From<StreamPosition> for RetentionWatermark {
    fn from(position: StreamPosition) -> Self {
        Self::DiscardBelow(position)
    }
}

impl From<CheckpointSequence> for RetentionWatermark {
    /// A checkpoint that has recorded no progress
    /// ([`CheckpointSequence::NONE`]) pins its streams to
    /// [`RetentionWatermark::RetainAll`].
    fn from(checkpoint: CheckpointSequence) -> Self {
        StreamPosition::try_new(checkpoint.as_u64()).map_or(Self::RetainAll, Self::DiscardBelow)
    }
}

impl std::fmt::Display for RetentionWatermark {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RetainAll => formatter.write_str("retain-all"),
            Self::DiscardBelow(position) => write!(formatter, "discard-below-{position}"),
        }
    }
}

/// Watermarks for the streams a [`RetentionWatermarksBuilder`] was shown.
///
/// Streams are keyed by snapshot id, which both execution paths set to the
/// stream id. Mapping that id onto the JetStream subject a purge would target
/// belongs to the caller, which owns the
/// [`StreamSubjectResolver`](crate::stream_store::StreamSubjectResolver) that
/// made the subject in the first place.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetentionWatermarks {
    streams: BTreeMap<String, RetentionWatermark>,
}

impl RetentionWatermarks {
    /// Returns the watermark for a stream id.
    ///
    /// A stream the report never saw returns [`RetentionWatermark::RetainAll`]:
    /// no snapshot means no evidence that any of its events are done with.
    pub fn watermark_for(&self, stream_id: &str) -> RetentionWatermark {
        self.streams
            .get(stream_id)
            .copied()
            .unwrap_or(RetentionWatermark::RetainAll)
    }

    /// Iterates every known stream id with its watermark, in key order.
    pub fn streams(&self) -> impl Iterator<Item = (&str, RetentionWatermark)> {
        self.streams.iter().map(|(id, watermark)| (id.as_str(), *watermark))
    }

    /// Returns the number of streams the report knows about.
    pub fn len(&self) -> usize {
        self.streams.len()
    }

    /// Returns whether the report knows about no streams at all.
    pub fn is_empty(&self) -> bool {
        self.streams.is_empty()
    }

    /// Returns the watermark that holds across every known stream.
    ///
    /// This is the input for a trim of the whole physical stream, and it is
    /// only sound when every logical stream on that physical stream is in the
    /// report: one absent, un-snapshotted stream is exactly the stream a
    /// physical trim would damage. Declare those ids with
    /// [`RetentionWatermarksBuilder::observe_stream_ids`] to have them counted.
    pub fn aggregate(&self) -> RetentionWatermark {
        self.streams
            .values()
            .copied()
            .min()
            .unwrap_or(RetentionWatermark::RetainAll)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct StreamCoverage {
    observed_by_types: usize,
    lowest_position: Option<StreamPosition>,
}

impl StreamCoverage {
    fn observe(&mut self, position: StreamPosition) {
        self.observed_by_types = self.observed_by_types.saturating_add(1);
        self.lowest_position = Some(match self.lowest_position {
            Some(lowest) => lowest.min(position),
            None => position,
        });
    }

    fn watermark(self, snapshot_types: usize) -> RetentionWatermark {
        if self.observed_by_types < snapshot_types {
            return RetentionWatermark::RetainAll;
        }

        self.lowest_position
            .map_or(RetentionWatermark::RetainAll, RetentionWatermark::DiscardBelow)
    }
}

/// Folds snapshots and checkpoints into a [`RetentionWatermarks`] report.
///
/// Snapshot types are folded one at a time because each carries its own
/// payload type, and a stream that one observed type has snapshotted while
/// another has not is pinned to [`RetentionWatermark::RetainAll`]: the type
/// with no snapshot for it would still replay that stream from the beginning.
///
/// Checkpoints are folded as a constraint shared by every stream. A checkpoint
/// records progress over the whole physical stream, not over one subject, so
/// it bounds every logical stream on it.
#[derive(Debug, Clone, Default)]
pub struct RetentionWatermarksBuilder {
    snapshot_types: usize,
    coverage: BTreeMap<String, StreamCoverage>,
    shared: Option<RetentionWatermark>,
}

impl RetentionWatermarksBuilder {
    /// Creates a builder that has observed nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds one snapshot type's snapshots, keyed by snapshot id.
    ///
    /// Takes the map [`read_snapshot_map`] returns. Each call counts as one
    /// snapshot type, so call it once per type rather than once per page.
    pub fn observe_snapshots<T>(mut self, snapshots: &BTreeMap<String, Snapshot<T>>) -> Self {
        self.snapshot_types = self.snapshot_types.saturating_add(1);
        for (snapshot_id, snapshot) in snapshots {
            self.coverage
                .entry(snapshot_id.clone())
                .or_default()
                .observe(snapshot.position);
        }
        self
    }

    /// Folds a checkpoint's progress as a constraint on every stream.
    pub fn observe_checkpoint(mut self, checkpoint: CheckpointSequence) -> Self {
        let watermark = RetentionWatermark::from(checkpoint);
        self.shared = Some(match self.shared {
            Some(shared) => shared.min(watermark),
            None => watermark,
        });
        self
    }

    /// Declares stream ids that exist, whether or not they have been
    /// snapshotted.
    ///
    /// Only ids the builder knows about can appear in the report, and only a
    /// report that covers every stream can produce a sound
    /// [`RetentionWatermarks::aggregate`].
    pub fn observe_stream_ids<I, S>(mut self, stream_ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for stream_id in stream_ids {
            self.coverage.entry(stream_id.into()).or_default();
        }
        self
    }

    /// Builds the report.
    pub fn build(self) -> RetentionWatermarks {
        let snapshot_types = self.snapshot_types;
        let shared = self.shared;
        RetentionWatermarks {
            streams: self
                .coverage
                .into_iter()
                .map(|(stream_id, coverage)| {
                    let watermark = coverage.watermark(snapshot_types);
                    let watermark = match shared {
                        Some(shared) => watermark.min(shared),
                        None => watermark,
                    };
                    (stream_id, watermark)
                })
                .collect(),
        }
    }
}

/// Reads the retention watermarks derivable from one snapshot type's bucket.
///
/// The convenience path for a deployment whose streams are covered by a single
/// snapshot type: it folds that type's snapshots and, when the config names
/// one, its checkpoint. A deployment with more than one snapshot type has to
/// drive [`RetentionWatermarksBuilder`] itself, because the missing type is
/// the one that makes the answer wrong.
pub async fn read_retention_watermarks<T, K>(
    bucket: &K,
    config: &NatsSnapshotConfig,
) -> Result<RetentionWatermarks, SnapshotStoreError<SnapshotDecodePayloadError<T>, SnapshotTypeError<T>>>
where
    T: SnapshotPayloadDecode + SnapshotType,
    SnapshotDecodePayloadError<T>: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvGet + JetStreamKvKeys + JetStreamKvEntry,
{
    let snapshots = read_snapshot_map::<T, K>(bucket).await?;
    let mut builder = RetentionWatermarksBuilder::new().observe_snapshots(&snapshots);

    if config.checkpoint_name().is_some() {
        let checkpoint = read_checkpoint::<T, K>(bucket, config).await.map_err(widen_payload)?;
        builder = builder.observe_checkpoint(CheckpointSequence::new(checkpoint));
    }

    Ok(builder.build())
}

/// Retypes an error raised by a checkpoint read, which cannot fail to code a
/// payload, so it can join errors from reads that can.
fn widen_payload<PayloadError, TypeError>(
    error: SnapshotStoreError<std::convert::Infallible, TypeError>,
) -> SnapshotStoreError<PayloadError, TypeError> {
    match error {
        SnapshotStoreError::Kv(source) => SnapshotStoreError::Kv(source),
        SnapshotStoreError::InvalidSnapshotKey { key } => SnapshotStoreError::InvalidSnapshotKey { key },
        SnapshotStoreError::MissingCheckpointName { snapshot_type } => {
            SnapshotStoreError::MissingCheckpointName { snapshot_type }
        }
        SnapshotStoreError::Codec(codec) => SnapshotStoreError::Codec(match codec {
            SnapshotCodecError::SnapshotType { source } => SnapshotCodecError::SnapshotType { source },
            SnapshotCodecError::EncodePayload { source } | SnapshotCodecError::DecodePayload { source } => {
                match source {}
            }
            SnapshotCodecError::EncodeEnvelope { source } => SnapshotCodecError::EncodeEnvelope { source },
            SnapshotCodecError::DecodeEnvelope { source } => SnapshotCodecError::DecodeEnvelope { source },
            SnapshotCodecError::UnexpectedSnapshotType { expected, actual } => {
                SnapshotCodecError::UnexpectedSnapshotType { expected, actual }
            }
        }),
    }
}

#[cfg(test)]
mod tests;
