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
//! # Event Replay Boundaries
//!
//! [`EventDecode`] can return [`EventDecodeOutcome::Skipped`] when a stored
//! event envelope does not belong to the decider's event set. The runtime treats
//! that as an ownership boundary, not a corrupt payload: replay applies only the
//! events the decider can own, while malformed payloads for owned event types
//! still surface as decode errors.
//!
//! Keeping that decision in the domain codec prevents storage adapters from
//! knowing every event enum or migration rule. Adapters only preserve the stored
//! [`EventType`] and bytes; application codecs decide whether those bytes are
//! part of the current decider's history.
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
//! # Ok::<(), trogon_decider_runtime::InvalidStreamPositionError>(())
//! ```
#![cfg_attr(
    any(test, feature = "test-support"),
    allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![cfg_attr(
    dylint_lib = "trogon_lints",
    expect(
        acyclic_modules,
        reason = "a stream is defined as an ordered sequence of the crate's events and an event carries the stream position it was read at"
    )
)]

/// Admission control gating how many command executions run concurrently.
pub mod admission;
/// Authorization gating which principal may run a command.
pub mod authorization;
mod command_id;
mod conflict_retry_limit;
mod constants;
/// Event envelopes and codec traits used by stream storage adapters.
pub mod event;
/// Command execution policies and runtime orchestration.
pub mod execution;
/// Metadata header value objects carried alongside event payloads.
pub mod headers;
/// In-memory test double for stream and snapshot storage contracts.
#[cfg(feature = "test-support")]
pub mod memory;
mod replay_bounds;
mod replay_chunk_size;
mod replay_limit;
/// Snapshot read/write contracts and payload codec traits.
pub mod snapshot;
/// Stream read/write contracts shared by event store backends.
pub mod stream;

pub use admission::{
    AdmissionLimit, AdmissionLimitError, CommandAdmission, ConcurrencyAdmission, OverloadedError, WithoutAdmission,
};
pub use authorization::{
    AuthorizationDeniedError, CommandAuthorizer, CommandPrincipal, DirectedPrincipal, DirectedPrincipalError,
    PrincipalClaim, PrincipalClaimError, PrincipalClaims, PrincipalId, PrincipalIdError, PrincipalKind,
    UnauthorizedError, WithoutAuthorization,
};
pub use command_id::CommandId;
pub use conflict_retry_limit::{ConflictRetryLimit, ConflictRetryLimitError};
pub use event::{Event, EventId, EventIdentity, StreamEvent};
#[cfg(any(test, feature = "test-support"))]
pub use execution::ImmediateSnapshotTaskScheduler;
pub use execution::{
    CommandError, CommandExecution, CommandResult, CommandSnapshotPolicy, DecideSnapshot,
    DiscardAndReplaySnapshotFailure, DrainableSnapshotTaskScheduler, ExecutionResult, ExecutionSnapshots,
    FailOnSnapshotFailure, FrequencySnapshot, LoadReplayError, LoadReplayResult, NoSnapshot, PreconditionConflictError,
    ReplayContext, ReplayLimitExceeded, RevisionAheadOfStream, SnapshotAheadOfStream, SnapshotDecision,
    SnapshotFailure, SnapshotFailureContext, SnapshotFailureDecision, SnapshotFailurePolicy, SnapshotPolicy,
    SnapshotTaskScheduler, Snapshots, TokioSnapshotTaskScheduler, WithoutSnapshotTaskScheduler, WithoutSnapshots,
    ensure_replay_within_limit, ensure_snapshot_not_ahead, read_stream_for_execution, resolve_write_precondition,
};
pub use headers::{FromEntriesError, HeaderName, HeaderNameError, HeaderValue, HeaderValueError, Headers};
#[cfg(feature = "test-support")]
pub use memory::{InMemoryStore, StreamAppendError};
pub use replay_bounds::{ReplayBounds, ReplayCursor};
pub use replay_chunk_size::{ReplayChunkSize, ReplayChunkSizeError};
pub use replay_limit::{ReplayLimit, ReplayLimitError};
pub use snapshot::{
    InvalidSnapshotTypeNameError, ReadSnapshotRequest, ReadSnapshotResponse, Snapshot, SnapshotPayloadData,
    SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotRead, SnapshotType, SnapshotTypeName, SnapshotWrite,
    WriteSnapshotRequest, WriteSnapshotResponse,
};
pub use stream::{
    AppendFailure, AppendStreamRequest, AppendStreamResponse, InvalidStreamPositionError, ReadAfterOverflowError,
    ReadFrom, ReadStreamRequest, ReadStreamResponse, StreamAppend, StreamPosition, StreamRead, StreamWritePrecondition,
};
#[cfg(feature = "test-support")]
pub use trogon_decider::testing;
#[cfg(feature = "test-support")]
pub use trogon_decider::testing::{History, TestCase, ThenError, ThenEvents, ThenExpectation};
pub use trogon_decider::{Act, ActBuilder, Decider, Decision, Events, SnapshotCadence, WritePrecondition};
pub use trogon_decider::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventPayloadError, EventType};
