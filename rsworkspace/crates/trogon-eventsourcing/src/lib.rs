#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod event;
mod execution;
pub mod nats;
pub mod snapshot;
mod stream;

pub use event::{
    Event, EventData, EventDecode, EventEncode, EventHeaders, EventHeadersFromEntriesError, EventId, EventIdentity,
    EventType, HeaderName, HeaderNameError, HeaderValue, HeaderValueError, StreamEvent,
};
#[cfg(any(test, feature = "test-support"))]
pub use execution::ImmediateSnapshotTaskScheduler;
pub use execution::{
    CommandError, CommandExecution, CommandResult, CommandSnapshotPolicy, ExecutionResult, FrequencySnapshot,
    DecideSnapshot, NoSnapshot, SnapshotAheadOfStream, SnapshotDecision, SnapshotPolicy,
    SnapshotTaskScheduler, Snapshots, TokioSnapshotTaskScheduler, WithoutSnapshotTaskScheduler, WithoutSnapshots,
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
    AppendStreamRequest, AppendStreamResponse, InvalidStreamPosition, ReadAfterOverflow, ReadFrom, ReadStreamRequest,
    ReadStreamResponse, StreamAppend, StreamPosition, StreamRead, StreamWritePrecondition,
};
#[cfg(feature = "test-support")]
pub use trogon_decider::testing;
#[cfg(feature = "test-support")]
pub use trogon_decider::testing::{History, TestCase, ThenError, ThenEvents, ThenExpectation};
pub use trogon_decider::{Act, ActBuilder, Decider, Decision, Events, WritePrecondition};
