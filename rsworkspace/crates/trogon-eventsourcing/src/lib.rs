#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod codec;
mod decision;
mod event;
mod event_id;
mod execution;
pub mod nats;
pub mod snapshot;
mod stream;
pub mod testing;

pub use codec::{CanonicalEventCodec, EventCodec, EventEnvelopeCodec, EventIdentity, EventType, JsonEventCodec};
pub use decision::{Act, Decide, Decision, NonEmpty, decide};
pub use event::{CodecError, EventData, RecordedEvent};
pub use event_id::EventId;
pub use execution::{
    CommandExecution, CommandFailure, CommandResult, CommandSnapshotPolicy, ExecutionResult, FrequencySnapshot,
    NoSnapshot, SnapshotDecision, SnapshotDecisionContext, SnapshotPolicy, Snapshots, WithoutSnapshots,
};
pub use nats::snapshot_store::{
    SnapshotStoreError, checkpoint_key, list_snapshots, maybe_advance_checkpoint, persist_snapshot_change,
    read_checkpoint, read_snapshot, read_snapshot_map, snapshot_key, write_checkpoint, write_snapshot,
};
pub use nats::streams::{
    StreamStoreError, TROGON_EVENT_TYPE, append_stream, read_stream, read_stream_range, record_stream_message,
};
pub use snapshot::{
    Snapshot, SnapshotChange, SnapshotRead, SnapshotSchema, SnapshotSink, SnapshotStoreConfig, SnapshotWrite,
};
pub use stream::{AppendOutcome, StreamAppend, StreamRead, StreamReadResult, StreamState};
pub use testing::{Decider, TestCase, ThenError, ThenEvents, ThenExpectation, Timeline, decider};
