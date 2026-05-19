#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod event;
mod execution;
pub mod nats;
pub mod snapshot;
mod stream;

pub use event::{
    EncodeEventError, Event, EventDecode, EventEncode, EventEncodeError, EventHeaders, EventHeadersError, EventId,
    EventIdentity, EventType, HeaderName, StreamEvent,
};
pub use execution::{
    BoxTask, CommandError, CommandExecution, CommandResult, CommandSnapshotPolicy, ExecutionResult, FrequencySnapshot,
    NoSnapshot, SnapshotDecision, SnapshotDecisionContext, SnapshotPolicy, Snapshots, WithoutSnapshotTaskScheduler,
    WithoutSnapshots, run_task_immediately, spawn_on_tokio,
};
pub use nats::{
    StreamStoreError, TROGON_EVENT_HEADER_PREFIX, TROGON_EVENT_TYPE, append_stream, read_stream, read_stream_range,
    record_stream_message,
};
pub use snapshot::{
    ReadSnapshotRequest, ReadSnapshotResponse, Snapshot, SnapshotRead, SnapshotType, SnapshotWrite,
    WriteSnapshotRequest, WriteSnapshotResponse,
};
pub use stream::{
    AppendStreamRequest, AppendStreamResponse, InvalidStreamPosition, ReadStreamRequest, ReadStreamResponse,
    StreamAppend, StreamPosition, StreamRead, StreamWritePrecondition,
};
#[cfg(feature = "test-support")]
pub use trogon_decider::testing;
#[cfg(feature = "test-support")]
pub use trogon_decider::testing::{History, TestCase, ThenError, ThenEvents, ThenExpectation};
pub use trogon_decider::{Act, ActBuilder, Decider, Decision, Events, WritePrecondition};
