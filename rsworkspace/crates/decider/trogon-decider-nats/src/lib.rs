//! NATS JetStream storage adapter for `trogon-decider-runtime`.
//!
//! The crate maps decider stream reads, appends, and snapshots onto JetStream
//! streams and Key/Value buckets. Callers provide a [`StreamSubjectResolver`]
//! so domain stream identifiers can keep their own subject layout while the
//! adapter handles stream positions, optimistic concurrency, event envelopes,
//! and snapshot persistence.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod store;

/// Durable pull-consumer message processing.
pub mod processor;
/// Generic catch-up driver for subject-filtered JetStream projections.
pub mod projector;
/// Idempotent create-or-open provisioning for JetStream streams and buckets.
pub mod provision;
/// Snapshot storage helpers backed by JetStream Key/Value.
pub mod snapshot_store;
/// Event stream storage helpers backed by JetStream streams.
pub mod stream_store;

pub use processor::{HandlerVerdict, MessageHandler, PoisonReason, Processor, ProcessorError, RedeliveryPolicy};
pub use projector::{
    CatchUpError, CatchUpOutcome, CheckpointSequence, ProjectionApply, ProjectionCheckpointStore, Projector,
};
pub use provision::{
    EnsureBucketError, EnsureStreamError, KvConfigMismatch, StreamConfigMismatch, ensure_bucket, ensure_stream,
};
pub use snapshot_store::{
    NatsSnapshotConfig, SnapshotChange, SnapshotCodecError, SnapshotKvError, SnapshotStoreError, checkpoint_key,
    list_snapshots, maybe_advance_checkpoint, persist_snapshot_change, read_checkpoint, read_snapshot,
    read_snapshot_map, snapshot_key, write_checkpoint, write_snapshot,
};
pub use store::{JetStreamStore, JetStreamStoreBuilder, JetStreamStoreError, OptimisticConcurrencyConflictError};
pub use stream_store::{
    PublishStreamError, ReadStreamError, StreamStoreError, StreamSubject, StreamSubjectResolver, SubjectState,
    TROGON_EVENT_HEADER_PREFIX, TROGON_EVENT_TYPE, append_stream, read_stream, read_stream_range,
    record_stream_message, subject_current_position,
};
