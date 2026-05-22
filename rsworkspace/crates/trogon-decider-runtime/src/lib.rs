//! Storage-neutral runtime contracts for Trogon deciders.
//!
//! This crate sits between pure decision logic from [`trogon_decider`] and the
//! storage adapters that persist or replay events. It owns the contracts whose
//! semantics must stay stable across backends: event envelopes, metadata
//! headers, stream reads, stream appends, snapshots, and stream positions.
//!
//! The crate deliberately avoids choosing a storage backend or deployment
//! topology. Applications compose those policies around these primitives so
//! adapters can remain thin translations to their native SDKs.
//!
//! # Command Execution
//!
//! [`CommandExecution`] is the runtime boundary for applying one [`Decider`]
//! command to one stream. It rebuilds command state from snapshots and stream
//! history, asks the decider for the next events, encodes those events into
//! storage envelopes, and appends them through the [`StreamAppend`] contract.
//!
//! The execution API keeps domain errors, codec errors, stream-read errors,
//! snapshot errors, and stream-append errors separated by phase. That
//! separation lets applications retry infrastructure failures without treating
//! domain rejection as a storage problem, and it lets storage adapters stay
//! focused on backend-specific read, append, and snapshot operations.
//!
//! # Position Semantics
//!
//! [`StreamPosition`] is a comparable stream high-watermark. It is not a
//! gapless revision, event count, or "next expected version". Callers may store
//! and compare it for concurrency and freshness checks, but should not do
//! arithmetic on it except through helpers such as [`ReadFrom::after`].
//!
//! # Example
//!
//! ```rust
//! use trogon_decider_runtime::{
//!     Event, EventId, Headers, StreamPosition, StreamWritePrecondition,
//! };
//! use uuid::Uuid;
//!
//! let event = Event {
//!     id: EventId::new(Uuid::now_v7()),
//!     r#type: "ExampleCreated".to_string(),
//!     content: br#"{"id":"example"}"#.to_vec(),
//!     headers: Headers::empty(),
//! };
//!
//! let observed = StreamPosition::try_new(1)?;
//! let precondition = StreamWritePrecondition::At(observed);
//! # let _ = (event, precondition);
//! # Ok::<(), trogon_decider_runtime::InvalidStreamPosition>(())
//! ```
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

/// Event envelopes and codec traits used by stream storage adapters.
pub mod event;
/// Command execution policies and runtime orchestration.
pub mod execution;
/// Metadata header value objects carried alongside event payloads.
pub mod headers;
/// Snapshot read/write contracts and payload codec traits.
pub mod snapshot;
/// Stream read/write contracts shared by event store backends.
pub mod stream;

pub use event::{
    Event, EventData, EventDecode, EventDecodeOutcome, EventEncode, EventId, EventIdentity, EventPayloadError,
    EventType, StreamEvent,
};
#[cfg(any(test, feature = "test-support"))]
pub use execution::ImmediateSnapshotTaskScheduler;
pub use execution::{
    CommandError, CommandExecution, CommandResult, CommandSnapshotPolicy, DecideSnapshot, ExecutionResult,
    FrequencySnapshot, NoSnapshot, SnapshotAheadOfStream, SnapshotDecision, SnapshotPolicy, SnapshotTaskScheduler,
    Snapshots, TokioSnapshotTaskScheduler, WithoutSnapshotTaskScheduler, WithoutSnapshots,
};
pub use headers::{FromEntriesError, HeaderName, HeaderNameError, HeaderValue, HeaderValueError, Headers};
pub use snapshot::{
    ReadSnapshotRequest, ReadSnapshotResponse, Snapshot, SnapshotPayloadData, SnapshotPayloadDecode,
    SnapshotPayloadEncode, SnapshotRead, SnapshotType, SnapshotWrite, WriteSnapshotRequest, WriteSnapshotResponse,
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
