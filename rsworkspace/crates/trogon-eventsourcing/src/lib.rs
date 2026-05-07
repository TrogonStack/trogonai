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

pub use codec::{CanonicalEventCodec, EventCodec, EventIdentity, EventType};
pub use decision::{Act, Decide, Decision, NonEmpty};
pub use event::{EncodeEventError, EventData, EventDataEncodeError, RecordedEvent};
pub use event_id::EventId;
pub use execution::{
    BoxTask, CommandExecution, CommandFailure, CommandResult, CommandSnapshotPolicy, ExecutionResult,
    FrequencySnapshot, NoSnapshot, SnapshotDecision, SnapshotDecisionContext, SnapshotPolicy, Snapshots,
    WithoutSnapshotTaskScheduler, WithoutSnapshots, run_task_immediately, spawn_on_tokio,
};
pub use nats::snapshot_store::{
    SnapshotStoreError, checkpoint_key, list_snapshots, maybe_advance_checkpoint, persist_snapshot_change,
    read_checkpoint, read_snapshot, read_snapshot_map, snapshot_key, write_checkpoint, write_snapshot,
};
pub use nats::{
    StreamStoreError, TROGON_EVENT_TYPE, append_stream, read_stream, read_stream_range, record_stream_message,
};
pub use snapshot::{
    ReadSnapshotRequest, ReadSnapshotResponse, Snapshot, SnapshotChange, SnapshotRead, SnapshotSchema,
    SnapshotStoreConfig, SnapshotWrite, WriteSnapshotRequest, WriteSnapshotResponse,
};
pub use stream::{
    AppendStreamRequest, AppendStreamResponse, InvalidStreamPosition, ReadStreamRequest, ReadStreamResponse,
    StreamAppend, StreamPosition, StreamRead, StreamState,
};
pub use testing::{Decider, TestCase, ThenError, ThenEvents, ThenExpectation, Timeline, decider};
