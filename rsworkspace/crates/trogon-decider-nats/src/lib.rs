//! NATS JetStream storage adapter for `trogon-decider-runtime`.
//!
//! The crate maps decider stream reads, appends, and snapshots onto JetStream
//! streams and Key/Value buckets. Callers provide a [`StreamSubjectResolver`]
//! so domain stream identifiers can keep their own subject layout while the
//! adapter handles stream positions, optimistic concurrency, event envelopes,
//! and snapshot persistence.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod store;

/// Snapshot storage helpers backed by JetStream Key/Value.
pub mod snapshot_store;
/// Event stream storage helpers backed by JetStream streams.
pub mod stream_store;

pub use snapshot_store::{
    NatsSnapshotConfig, SnapshotChange, SnapshotStoreError, checkpoint_key, list_snapshots, maybe_advance_checkpoint,
    persist_snapshot_change, read_checkpoint, read_snapshot, read_snapshot_map, snapshot_key, write_checkpoint,
    write_snapshot,
};
pub use store::{JetStreamStore, JetStreamStoreBuilder, JetStreamStoreError, OptimisticConcurrencyConflictError};
pub use stream_store::{
    StreamStoreError, StreamSubject, StreamSubjectResolver, SubjectState, TROGON_EVENT_HEADER_PREFIX,
    TROGON_EVENT_TYPE, append_stream, read_stream, read_stream_range, record_stream_message, subject_current_position,
};
