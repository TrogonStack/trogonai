//! Command execution against a compiled WASM decider module.
//!
//! This mirrors [`trogon_decider_runtime::execution::CommandExecution`]
//! phase-by-phase, but the domain core lives inside a guest component instead
//! of a native `trogon_decider::Decider` implementation. Static evolve/decide
//! functions and a per-decider write precondition const cannot address session
//! state living behind `&mut wasmtime::Store` and a
//! `wasmtime::component::ResourceAny` handle, so this module reimplements the
//! same replay, decide, encode, and append flow directly against the WIT
//! `session` resource.
//!
//! # Snapshot and decide ordering
//!
//! The guest macro's generated `decide` export only reads session state; it
//! never folds its own newly decided events back into that state (verified
//! against `trogon-decider-guest-macros`: `evolve` mutates the session's
//! `RefCell<State>`, while `decide` only borrows it). A snapshot captured
//! immediately after `decide` would therefore silently drop this command's
//! own events. This execution evolves the session with the decided events
//! before calling `snapshot`, so a session resumed from that snapshot ends up
//! in the same state as a full replay from the beginning of the stream.
//!
//! # Guest calls run off the async executor
//!
//! Every guest export call is fuel-metered and wall-clock bounded (see
//! [`crate::WasmDeciderEngine::arm_guest_call`]), but a synchronous guest call
//! still occupies whatever thread calls it for up to that budget. Running it
//! inline on the async executor would stall every other task that executor
//! thread is responsible for. Both [`WithoutSnapshotStore`] and
//! [`WithSnapshotStore`] executions instead move each contiguous run of guest
//! calls onto a blocking thread pool via [`spawn_guest`], resuming the async
//! task only for real I/O (`StreamRead`, `StreamAppend`, `SnapshotRead`).

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use opentelemetry::metrics::{Counter, Histogram};
use opentelemetry::{KeyValue, global};
use thiserror::Error;
use trogon_decider_runtime::{
    AppendFailure, AppendStreamRequest, CommandAdmission, CommandAuthorizer, CommandId, CommandPrincipal,
    ConflictRetryLimit, DiscardAndReplaySnapshotFailure, Event, EventId, FailOnSnapshotFailure, Headers,
    OverloadedError, PreconditionConflictError, ReadAfterOverflowError, ReadFrom, ReadSnapshotRequest,
    ReadStreamRequest, ReplayBounds, ReplayChunkSize, ReplayCursor, ReplayLimit, ReplayLimitExceeded, Snapshot,
    SnapshotAheadOfStream, SnapshotCadence, SnapshotFailure, SnapshotFailureDecision, SnapshotRead,
    SnapshotTaskScheduler, SnapshotWrite, StreamAppend, StreamEvent, StreamPosition, StreamRead, UnauthorizedError,
    WithoutAdmission, WithoutAuthorization, WritePrecondition, WriteSnapshotRequest, ensure_replay_within_limit,
    read_stream_for_execution, resolve_write_precondition,
};
use trogon_decider_wit::host::{self, AnyEnvelope, CommandEnvelope, DecideError};
use trogon_semconv::{attribute, metric, span};
use trogon_std::NowV7;
use wasmtime::Store;

use crate::constants::METER_NAME;
use crate::{
    GuestDomainError, OpaqueSnapshotPayload, WasmCommandSpec, WasmDeciderEngine, WasmDeciderModule, WasmSnapshotId,
};

struct WasmExecutionMetrics {
    execution_duration: Histogram<f64>,
    fuel_consumed: Histogram<u64>,
    traps: Counter<u64>,
    conflict_retries: Counter<u64>,
}

impl WasmExecutionMetrics {
    fn new() -> Self {
        let meter = global::meter(METER_NAME);
        Self {
            execution_duration: metric::build_decider_wasm_execution_duration(&meter),
            fuel_consumed: metric::build_decider_wasm_fuel_consumed(&meter),
            traps: metric::build_decider_wasm_traps(&meter),
            conflict_retries: metric::build_decider_command_conflict_retries(&meter),
        }
    }
}

static METRICS: OnceLock<WasmExecutionMetrics> = OnceLock::new();

fn metrics() -> &'static WasmExecutionMetrics {
    METRICS.get_or_init(WasmExecutionMetrics::new)
}

/// Module, version, and command type shared by every guest phase span and
/// metric recorded for one command execution.
#[derive(Debug, Clone)]
struct GuestPhaseContext {
    module_name: String,
    module_version: String,
    command_type: String,
}

impl GuestPhaseContext {
    fn new(module: &WasmDeciderModule, command: &CommandEnvelope) -> Self {
        Self {
            module_name: module.name().to_string(),
            module_version: module.version().to_string(),
            command_type: command.type_.clone(),
        }
    }
}

fn phase_attributes(context: &GuestPhaseContext, phase: attribute::GuestPhase) -> [KeyValue; 4] {
    [
        KeyValue::new(attribute::MODULE_NAME, context.module_name.clone()),
        KeyValue::new(attribute::MODULE_VERSION, context.module_version.clone()),
        KeyValue::new(attribute::COMMAND_TYPE, context.command_type.clone()),
        KeyValue::new(attribute::GUEST_PHASE, phase.as_str()),
    ]
}

fn record_phase_metrics(
    context: &GuestPhaseContext,
    phase: attribute::GuestPhase,
    duration: Duration,
    fuel_consumed: u64,
) {
    let attributes = phase_attributes(context, phase);
    metrics().execution_duration.record(duration.as_secs_f64(), &attributes);
    metrics().fuel_consumed.record(fuel_consumed, &attributes);
}

fn record_phase_trap(
    context: &GuestPhaseContext,
    phase: attribute::GuestPhase,
    classification: attribute::TrapClassification,
) {
    let mut attributes = phase_attributes(context, phase).to_vec();
    attributes.push(KeyValue::new(attribute::TRAP_CLASSIFICATION, classification.as_str()));
    metrics().traps.add(1, &attributes);
}

fn phase_fuel_consumed<T>(store: &Store<T>, fuel_budget: u64) -> u64 {
    store
        .get_fuel()
        .map_or(fuel_budget, |remaining| fuel_budget.saturating_sub(remaining))
}

fn trap_classification(error: &wasmtime::Error) -> attribute::TrapClassification {
    if is_epoch_deadline_exceeded(error) {
        attribute::TrapClassification::DeadlineExceeded
    } else {
        attribute::TrapClassification::Trap
    }
}

/// Result of a successful WASM command execution.
#[derive(Debug, Clone)]
pub struct WasmExecutionResult {
    /// The stream high-watermark after the command append completed.
    pub stream_position: StreamPosition,
    /// Domain events emitted by the guest decider and appended to the stream.
    ///
    /// The appended [`Event`]s rather than the guest's raw envelopes, so the
    /// event ids the host minted for the append survive into the result. A
    /// caller applying this result and also consuming the event stream needs
    /// those ids to recognize the two as the same events.
    pub events: Vec<Event>,
}

/// Error taxonomy for a WASM command execution attempt.
#[derive(Debug, Error)]
pub enum WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError> {
    /// The guest rejected the command as a domain failure.
    #[error("command rejected: {0}")]
    Rejected(#[source] GuestDomainError),
    /// The guest faulted while deciding the command.
    #[error("command faulted: {0}")]
    Faulted(#[source] GuestDomainError),
    /// The guest's `decide` call reported success but returned zero events,
    /// violating the WIT `decide` contract's non-empty invariant (see
    /// `world.wit`). Rejected before the session is folded, snapshotted, or
    /// appended, the same as [`Self::Rejected`] and [`Self::Faulted`].
    #[error("command decided no events")]
    EmptyDecision,
    /// The guest could not evolve session state from a replayed or decided event.
    #[error("command state evolution failed: {0}")]
    Evolve(#[source] GuestDomainError),
    /// The guest could not compute the command's stream id.
    #[error("command stream id resolution failed: {0}")]
    StreamId(#[source] GuestDomainError),
    /// A guest call trapped (fuel exhaustion, memory limit, or ABI failure).
    ///
    /// Distinct from [`Self::Faulted`]: a trap is a host-level wasmtime
    /// failure, not a domain error the guest chose to report. Distinct from
    /// [`Self::DeadlineExceeded`]: this variant is every other trap cause.
    #[error("guest call trapped")]
    Trap(#[source] wasmtime::Error),
    /// A guest call exceeded its wall-clock epoch deadline.
    ///
    /// Wasmtime's epoch-based interruption raised this instead of the guest
    /// exhausting its fuel budget first, meaning the guest was still running
    /// but too slowly. Distinguished from [`Self::Trap`] so callers can tell
    /// a hung guest apart from every other host-level trap.
    #[error("guest call exceeded its wall-clock deadline")]
    DeadlineExceeded(#[source] wasmtime::Error),
    /// The module could not be instantiated for this command.
    #[error("failed to instantiate wasm component")]
    Instantiate(#[source] wasmtime::Error),
    /// Snapshot loading failed before replaying stream history.
    #[error("command snapshot read failed: {0}")]
    ReadSnapshot(#[source] ReadSnapshotError),
    /// Stream history loading failed.
    #[error("command stream read failed: {0}")]
    ReadStream(#[source] ReadStreamError),
    /// Appending the decided events failed after the command was accepted.
    #[error("command stream append failed: {0}")]
    Append(#[source] AppendStreamError),
    /// The loaded snapshot claims a position newer than the stream can prove.
    #[error("{0}")]
    SnapshotAheadOfStream(SnapshotAheadOfStream),
    /// The snapshot's recorded position cannot be advanced (u64 overflow).
    #[error("{0}")]
    ReadAfterOverflow(#[source] ReadAfterOverflowError),
    /// The stream read for this command returned more events than the
    /// configured [`ReplayLimit`] allows to replay into the guest session.
    #[error("{0}")]
    ReplayLimitExceeded(ReplayLimitExceeded),
    /// The caller's expected revision cannot be reconciled with the write
    /// precondition the module declares for this command.
    #[error("{0}")]
    PreconditionConflict(#[source] PreconditionConflictError),
    /// The blocking task running guest calls panicked or was cancelled.
    #[error("guest execution task failed")]
    Blocking(#[source] tokio::task::JoinError),
    /// The configured [`CommandAdmission`] limiter had no capacity, so the
    /// command was shed before any guest store was created.
    #[error("{0}")]
    Overloaded(OverloadedError),
    /// The configured [`CommandAuthorizer`] refused this principal, or none was
    /// supplied, so the command was denied before any guest store was created.
    #[error("{0}")]
    Unauthorized(#[source] UnauthorizedError),
}

/// Module identity, command type, and stream id a [`WasmSnapshotFailurePolicy`]
/// inspects before deciding how to react to a snapshot a [`WasmCommandExecution`]
/// cannot trust.
///
/// Mirrors [`trogon_decider_runtime::SnapshotFailureContext`], adapted for the
/// WASM boundary: there is no typed `Decider` to hand the policy, so this
/// carries the identity the wasm execution actually has in hand at this point
/// instead of a command reference.
#[derive(Debug)]
pub struct WasmSnapshotFailureContext<'a, ReadSnapshotError> {
    /// Name of the module executing the command that triggered this failure.
    pub module_name: &'a str,
    /// Version of the module executing the command that triggered this failure.
    pub module_version: &'a str,
    /// Wire type URL of the command that triggered this failure.
    pub command_type: &'a str,
    /// Stream id the command resolved before loading its snapshot.
    pub stream_id: &'a str,
    /// The failure the policy must decide how to handle.
    pub failure: SnapshotFailure<'a, ReadSnapshotError>,
}

/// Chooses how a [`WasmCommandExecution`] reacts to a snapshot it cannot trust.
///
/// Mirrors [`trogon_decider_runtime::SnapshotFailurePolicy`] for the WASM
/// boundary, which has no typed `Decider` to bound the policy on.
/// [`FailOnSnapshotFailure`] keeps today's behavior of failing the command;
/// [`DiscardAndReplaySnapshotFailure`] discards the bad snapshot and replays
/// from the beginning of the stream. Both are reused directly from
/// `trogon_decider_runtime` since neither carries decider-specific state.
pub trait WasmSnapshotFailurePolicy<ReadSnapshotError> {
    /// Decides how the command execution should react to the given snapshot failure.
    fn decide_snapshot_failure(
        &self,
        context: WasmSnapshotFailureContext<'_, ReadSnapshotError>,
    ) -> SnapshotFailureDecision;
}

impl<ReadSnapshotError> WasmSnapshotFailurePolicy<ReadSnapshotError> for FailOnSnapshotFailure {
    fn decide_snapshot_failure(
        &self,
        _context: WasmSnapshotFailureContext<'_, ReadSnapshotError>,
    ) -> SnapshotFailureDecision {
        SnapshotFailureDecision::Fail
    }
}

impl<ReadSnapshotError> WasmSnapshotFailurePolicy<ReadSnapshotError> for DiscardAndReplaySnapshotFailure {
    fn decide_snapshot_failure(
        &self,
        _context: WasmSnapshotFailureContext<'_, ReadSnapshotError>,
    ) -> SnapshotFailureDecision {
        SnapshotFailureDecision::DiscardAndReplay
    }
}

/// Marker type used before a snapshot store is attached to an execution.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WithoutSnapshotStore;

/// Snapshot store, best-effort task scheduler, and snapshot failure policy
/// attached to an execution.
pub struct WithSnapshotStore<'a, S, Sched, F = FailOnSnapshotFailure> {
    store: &'a S,
    task_scheduler: &'a Sched,
    failure_policy: F,
}

/// Builder for one command execution against a [`WasmDeciderModule`].
pub struct WasmCommandExecution<'a, E, Snapshots, G, A = WithoutAdmission, Auth = WithoutAuthorization> {
    module: &'a WasmDeciderModule,
    event_store: &'a E,
    command: &'a CommandEnvelope,
    snapshots: Snapshots,
    expected_revision: Option<StreamPosition>,
    replay_limit: Option<ReplayLimit>,
    replay_chunk_size: Option<ReplayChunkSize>,
    conflict_retry_limit: Option<ConflictRetryLimit>,
    snapshot_cadence: Option<SnapshotCadence>,
    headers: Headers,
    command_id: Option<CommandId>,
    event_id_generator: G,
    admission: A,
    principal: Option<CommandPrincipal>,
    authorizer: Auth,
}

impl<'a, E> WasmCommandExecution<'a, E, WithoutSnapshotStore, trogon_std::UuidV7Generator> {
    /// Starts building an execution for the given module, event store, and command.
    pub fn new(module: &'a WasmDeciderModule, event_store: &'a E, command: &'a CommandEnvelope) -> Self {
        Self {
            module,
            event_store,
            command,
            snapshots: WithoutSnapshotStore,
            expected_revision: None,
            replay_limit: None,
            replay_chunk_size: None,
            conflict_retry_limit: None,
            snapshot_cadence: None,
            headers: Headers::empty(),
            command_id: None,
            event_id_generator: trogon_std::UuidV7Generator,
            admission: WithoutAdmission,
            principal: None,
            authorizer: WithoutAuthorization,
        }
    }
}

impl<'a, E, G, A, Auth> WasmCommandExecution<'a, E, WithoutSnapshotStore, G, A, Auth> {
    /// Attaches a snapshot store and best-effort snapshot task scheduler.
    ///
    /// Defaults to [`FailOnSnapshotFailure`], which fails the command on an
    /// untrusted snapshot exactly as before this policy existed. Chain
    /// [`WasmCommandExecution::with_snapshot_failure_policy`] to recover
    /// instead.
    pub fn with_snapshot_store<S, Sched>(
        self,
        snapshot_store: &'a S,
        snapshot_task_scheduler: &'a Sched,
    ) -> WasmCommandExecution<'a, E, WithSnapshotStore<'a, S, Sched, FailOnSnapshotFailure>, G, A, Auth> {
        WasmCommandExecution {
            module: self.module,
            event_store: self.event_store,
            command: self.command,
            snapshots: WithSnapshotStore {
                store: snapshot_store,
                task_scheduler: snapshot_task_scheduler,
                failure_policy: FailOnSnapshotFailure,
            },
            expected_revision: self.expected_revision,
            replay_limit: self.replay_limit,
            replay_chunk_size: self.replay_chunk_size,
            conflict_retry_limit: self.conflict_retry_limit,
            snapshot_cadence: self.snapshot_cadence,
            command_id: self.command_id,
            headers: self.headers,
            event_id_generator: self.event_id_generator,
            admission: self.admission,
            principal: self.principal,
            authorizer: self.authorizer,
        }
    }
}

impl<'a, E, S, Sched, F, G, A, Auth> WasmCommandExecution<'a, E, WithSnapshotStore<'a, S, Sched, F>, G, A, Auth> {
    /// Sets how this execution reacts to a snapshot it cannot trust.
    ///
    /// Defaults to [`FailOnSnapshotFailure`], which fails the command exactly
    /// as before this policy existed. Use [`DiscardAndReplaySnapshotFailure`]
    /// or a custom [`WasmSnapshotFailurePolicy`] to recover instead.
    pub fn with_snapshot_failure_policy<NextF>(
        self,
        failure_policy: NextF,
    ) -> WasmCommandExecution<'a, E, WithSnapshotStore<'a, S, Sched, NextF>, G, A, Auth> {
        WasmCommandExecution {
            module: self.module,
            event_store: self.event_store,
            command: self.command,
            snapshots: WithSnapshotStore {
                store: self.snapshots.store,
                task_scheduler: self.snapshots.task_scheduler,
                failure_policy,
            },
            expected_revision: self.expected_revision,
            replay_limit: self.replay_limit,
            replay_chunk_size: self.replay_chunk_size,
            conflict_retry_limit: self.conflict_retry_limit,
            snapshot_cadence: self.snapshot_cadence,
            command_id: self.command_id,
            headers: self.headers,
            event_id_generator: self.event_id_generator,
            admission: self.admission,
            principal: self.principal,
            authorizer: self.authorizer,
        }
    }
}

impl<'a, E, Snapshots, G, A, Auth> WasmCommandExecution<'a, E, Snapshots, G, A, Auth> {
    /// Records the stream revision a client observed before issuing this command.
    ///
    /// Strengthens the guard the module declares in its descriptor rather than replacing it: the
    /// append is pinned to that exact revision, so a stream that has advanced since the client read
    /// it rejects the write. A command declaring [`WritePrecondition::NoStream`] fails with
    /// [`WasmCommandError::PreconditionConflict`] instead, because it can only create a stream that
    /// has no revision yet.
    pub fn with_expected_revision<R>(mut self, expected_revision: R) -> Self
    where
        R: Into<Option<StreamPosition>>,
    {
        self.expected_revision = expected_revision.into();
        self
    }

    /// Caps how many stream events one execution may replay into the guest session.
    ///
    /// Without a limit, a long stream is handed to a single fuel-bounded guest `evolve` call, so it
    /// fails as an `OutOfFuel` trap that is indistinguishable from a buggy guest. With a limit the
    /// read is bounded to `limit + 1` events and the command fails with
    /// [`WasmCommandError::ReplayLimitExceeded`] before any of them reach the guest, matching
    /// [`trogon_decider_runtime::CommandExecution`].
    pub fn with_replay_limit<L>(mut self, replay_limit: L) -> Self
    where
        L: Into<Option<ReplayLimit>>,
    {
        self.replay_limit = replay_limit.into();
        self
    }

    /// Walks the replay in chunks of at most `replay_chunk_size` events instead of reading the
    /// whole stream at once.
    ///
    /// Mirrors
    /// [`CommandExecution::with_replay_chunk_size`](trogon_decider_runtime::CommandExecution::with_replay_chunk_size),
    /// and buys the guest path one thing more than the native one: each chunk is folded by its own
    /// `evolve` call, so the fuel and epoch budget [`replay_fuel`] scales is per chunk rather than
    /// per stream. A stream long enough to exhaust a single call's budget no longer has to.
    pub fn with_replay_chunk_size<C>(mut self, replay_chunk_size: C) -> Self
    where
        C: Into<Option<ReplayChunkSize>>,
    {
        self.replay_chunk_size = replay_chunk_size.into();
        self
    }

    /// Retries this execution in place, up to `conflict_retry_limit` times, when another writer
    /// beat it to the stream.
    ///
    /// Mirrors [`CommandExecution::with_conflict_retry`](trogon_decider_runtime::CommandExecution::with_conflict_retry),
    /// including what it refuses to retry: the event store has to classify the failure as
    /// [`AppendFailure::WriteConflict`], the module's descriptor has to declare
    /// [`WritePrecondition::StreamUnchanged`] for this command, and no
    /// [`with_expected_revision`](Self::with_expected_revision) may be set. Anything else asserts
    /// something a second read cannot change.
    ///
    /// A retry is a whole new round: a fresh guest session, a fresh replay, and a fresh decide. The
    /// cost of one is roughly the cost of the command, which is what the limit is there to bound.
    pub fn with_conflict_retry<L>(mut self, conflict_retry_limit: L) -> Self
    where
        L: Into<Option<ConflictRetryLimit>>,
    {
        self.conflict_retry_limit = conflict_retry_limit.into();
        self
    }

    /// Overrides the snapshot cadence the module declares for this command.
    ///
    /// The descriptor's cadence is what keeps this path in parity with
    /// [`trogon_decider_runtime::CommandExecution`], so prefer leaving it alone. This exists for
    /// the same reason the native path accepts an explicit
    /// [`Snapshots`](trogon_decider_runtime::Snapshots) policy instead of the decider's own:
    /// cadence is a cost trade rather than a correctness one, so a host that knows its storage
    /// costs better than the module's author does may set its own.
    pub fn with_snapshot_cadence<S>(mut self, snapshot_cadence: S) -> Self
    where
        S: Into<Option<SnapshotCadence>>,
    {
        self.snapshot_cadence = snapshot_cadence.into();
        self
    }

    /// Attaches metadata headers propagated onto every appended event.
    pub fn with_headers(mut self, headers: Headers) -> Self {
        self.headers = headers;
        self
    }

    /// Records the identity of the delivery that carries this command.
    ///
    /// Every event this execution appends then derives its [`EventId`] from that identity, so a
    /// redelivered command produces byte-identical ids and a storage adapter that dedupes on event
    /// identity can recognize the retry. Without one, ids come from
    /// [`with_event_id_generator`](Self::with_event_id_generator) and each attempt looks new.
    ///
    /// The id has to come from the transport, not from this builder: a value generated here would
    /// be as fresh on the retry as the UUIDv7 it replaces.
    pub fn with_command_id<I>(mut self, command_id: I) -> Self
    where
        I: Into<Option<CommandId>>,
    {
        self.command_id = command_id.into();
        self
    }

    /// Overrides the event id generator (defaults to [`trogon_std::UuidV7Generator`]).
    ///
    /// Only reached for events that no [`with_command_id`](Self::with_command_id) derivation covers.
    pub fn with_event_id_generator<NextG>(
        self,
        event_id_generator: NextG,
    ) -> WasmCommandExecution<'a, E, Snapshots, NextG, A, Auth>
    where
        NextG: NowV7,
    {
        WasmCommandExecution {
            module: self.module,
            event_store: self.event_store,
            command: self.command,
            snapshots: self.snapshots,
            expected_revision: self.expected_revision,
            replay_limit: self.replay_limit,
            replay_chunk_size: self.replay_chunk_size,
            conflict_retry_limit: self.conflict_retry_limit,
            snapshot_cadence: self.snapshot_cadence,
            command_id: self.command_id,
            headers: self.headers,
            event_id_generator,
            admission: self.admission,
            principal: self.principal,
            authorizer: self.authorizer,
        }
    }

    /// Gates this execution on a [`CommandAdmission`] limiter.
    ///
    /// Defaults to [`WithoutAdmission`]: execution is unbounded, exactly as it
    /// was before admission control existed. Once set, the limiter is
    /// consulted before the guest store is created, so a command it cannot
    /// admit fails with [`WasmCommandError::Overloaded`] without reserving any
    /// of the linear memory a wasm store would.
    ///
    /// The bound worth choosing here is
    /// `limit x WasmEngineConfig::max_memory_bytes` against the host's memory
    /// budget, which is why no default exists.
    pub fn with_admission<NextA>(self, admission: NextA) -> WasmCommandExecution<'a, E, Snapshots, G, NextA, Auth>
    where
        NextA: CommandAdmission,
    {
        WasmCommandExecution {
            module: self.module,
            event_store: self.event_store,
            command: self.command,
            snapshots: self.snapshots,
            expected_revision: self.expected_revision,
            replay_limit: self.replay_limit,
            replay_chunk_size: self.replay_chunk_size,
            conflict_retry_limit: self.conflict_retry_limit,
            snapshot_cadence: self.snapshot_cadence,
            command_id: self.command_id,
            headers: self.headers,
            event_id_generator: self.event_id_generator,
            admission,
            principal: self.principal,
            authorizer: self.authorizer,
        }
    }

    /// Names the [`CommandPrincipal`] this command is executed on behalf of.
    ///
    /// Only read by a configured [`CommandAuthorizer`]. It is not persisted:
    /// [`Headers`] remains the envelope metadata contract, so an application that wants an audit
    /// trail of who acted derives its own header from the principal and sets it with
    /// [`with_headers`](Self::with_headers).
    ///
    /// Setting this without an authorizer configured changes nothing, and configuring an authorizer
    /// without setting this fails every execution with [`UnauthorizedError::MissingPrincipal`].
    pub fn with_principal<P>(mut self, principal: P) -> Self
    where
        P: Into<Option<CommandPrincipal>>,
    {
        self.principal = principal.into();
        self
    }

    /// Gates this execution on a [`CommandAuthorizer`].
    ///
    /// Defaults to [`WithoutAuthorization`]: any caller able to build an execution may run any
    /// command, exactly as it was before authorization existed. Once set, the authorizer is
    /// consulted after the admission permit and before the guest store is created, so a command it
    /// refuses fails with [`WasmCommandError::Unauthorized`] without instantiating the module,
    /// spending guest fuel, or reading anything.
    ///
    /// The authorizer is handed the [`CommandEnvelope`] rather than a typed command, and never the
    /// target stream: on this path the stream id is a value the guest computes, so it does not
    /// exist until the guest has already run. See
    /// [ADR#0026](https://github.com/TrogonStack/trogonai/blob/main/docs/adr/0026-command-authorization-principal.md).
    pub fn with_authorizer<NextAuth>(
        self,
        authorizer: NextAuth,
    ) -> WasmCommandExecution<'a, E, Snapshots, G, A, NextAuth>
    where
        NextAuth: CommandAuthorizer<CommandEnvelope>,
    {
        WasmCommandExecution {
            module: self.module,
            event_store: self.event_store,
            command: self.command,
            snapshots: self.snapshots,
            expected_revision: self.expected_revision,
            replay_limit: self.replay_limit,
            replay_chunk_size: self.replay_chunk_size,
            conflict_retry_limit: self.conflict_retry_limit,
            snapshot_cadence: self.snapshot_cadence,
            command_id: self.command_id,
            headers: self.headers,
            event_id_generator: self.event_id_generator,
            admission: self.admission,
            principal: self.principal,
            authorizer,
        }
    }
}

/// The first chunk of prior stream events to replay, the stream's current
/// position, and the raw snapshot bytes passed to the guest session
/// constructor.
///
/// The events stay [`StreamEvent`]s rather than guest envelopes because a
/// chunked replay walks from the position of the last event it folded, which
/// an envelope no longer carries.
pub struct ReplayContext {
    stream_events: Vec<StreamEvent>,
    current_position: Option<StreamPosition>,
    snapshot_bytes: Option<Vec<u8>>,
}

mod sealed;

/// Why a sandboxed command execution could not assemble the session state and history to replay.
///
/// Widened into the matching [`WasmCommandError`] variants by the skeleton that called it.
#[derive(Debug, Error)]
pub enum WasmLoadReplayError<ReadSnapshotError, ReadStreamError> {
    /// The snapshot store could not be read, and the failure policy chose to fail.
    #[error("{0}")]
    ReadSnapshot(#[source] ReadSnapshotError),
    /// The stream could not be read.
    #[error("{0}")]
    ReadStream(#[source] ReadStreamError),
    /// A snapshot claimed a position the stream cannot prove, and the failure policy chose to fail.
    #[error("{0}")]
    SnapshotAheadOfStream(SnapshotAheadOfStream),
    /// A snapshot's position cannot be advanced to the first event after it.
    #[error("{0}")]
    ReadAfterOverflow(#[source] ReadAfterOverflowError),
}

impl<ReadSnapshotError, ReadStreamError, AppendStreamError>
    From<WasmLoadReplayError<ReadSnapshotError, ReadStreamError>>
    for WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>
{
    fn from(error: WasmLoadReplayError<ReadSnapshotError, ReadStreamError>) -> Self {
        match error {
            WasmLoadReplayError::ReadSnapshot(error) => Self::ReadSnapshot(error),
            WasmLoadReplayError::ReadStream(error) => Self::ReadStream(error),
            WasmLoadReplayError::SnapshotAheadOfStream(ahead) => Self::SnapshotAheadOfStream(ahead),
            WasmLoadReplayError::ReadAfterOverflow(error) => Self::ReadAfterOverflow(error),
        }
    }
}

/// What [`WasmExecutionSnapshots::load_replay_context`] hands back to the execution skeleton.
pub type WasmLoadReplayResult<ReadSnapshotError, ReadStreamError> =
    Result<ReplayContext, WasmLoadReplayError<ReadSnapshotError, ReadStreamError>>;

/// The snapshot half of one sandboxed command execution.
///
/// The counterpart of [`trogon_decider_runtime::ExecutionSnapshots`] for the guest path, and it
/// exists for the same reason: instantiation, the stream read, the guest session, the append, and
/// the write precondition run once for both configurations, so a change to any of them is a change
/// in one place. Only what a snapshot store contributes varies.
pub trait WasmExecutionSnapshots: sealed::Sealed {
    /// How a snapshot read can fail. [`Infallible`](std::convert::Infallible) when there is no
    /// store to read from.
    type ReadError;

    /// Loads the snapshot to resume the guest session from and the stream events recorded after it.
    fn load_replay_context<E>(
        &self,
        module: &WasmDeciderModule,
        command: &CommandEnvelope,
        event_store: &E,
        stream_id: &str,
        bounds: ReplayBounds,
    ) -> impl Future<Output = WasmLoadReplayResult<Self::ReadError, <E as StreamRead<str>>::Error>> + Send
    where
        E: StreamRead<str>;

    /// Narrows the cadence the module declares to the one this configuration can honor.
    ///
    /// Without a store nothing would ever read a snapshot, so the guest is not asked to fold its
    /// decided events back in or to serialize itself: both would burn fuel producing bytes that go
    /// nowhere.
    fn snapshot_cadence(&self, declared: SnapshotCadence) -> SnapshotCadence;

    /// Persists the bytes the guest serialized itself into, if there is a store to persist them to.
    fn store(&self, module: &WasmDeciderModule, stream_id: &str, stream_position: StreamPosition, bytes: Vec<u8>);
}

impl sealed::Sealed for WithoutSnapshotStore {}

impl WasmExecutionSnapshots for WithoutSnapshotStore {
    type ReadError = std::convert::Infallible;

    async fn load_replay_context<E>(
        &self,
        _module: &WasmDeciderModule,
        _command: &CommandEnvelope,
        event_store: &E,
        stream_id: &str,
        bounds: ReplayBounds,
    ) -> WasmLoadReplayResult<Self::ReadError, <E as StreamRead<str>>::Error>
    where
        E: StreamRead<str>,
    {
        let stream_read = read_stream_for_execution(
            event_store,
            ReadStreamRequest {
                stream_id,
                from: ReadFrom::Beginning,
            },
            bounds.read_bound(0),
        )
        .await
        .map_err(WasmLoadReplayError::ReadStream)?;

        Ok(ReplayContext {
            stream_events: stream_read.events,
            current_position: stream_read.current_position,
            snapshot_bytes: None,
        })
    }

    fn snapshot_cadence(&self, _declared: SnapshotCadence) -> SnapshotCadence {
        SnapshotCadence::Never
    }

    fn store(&self, _module: &WasmDeciderModule, _stream_id: &str, _position: StreamPosition, _bytes: Vec<u8>) {}
}

impl<S, Sched, F> WithSnapshotStore<'_, S, Sched, F> {
    /// Builds the context a [`WasmSnapshotFailurePolicy`] inspects for one
    /// snapshot failure this execution encountered.
    fn snapshot_failure_context<'ctx, ReadSnapshotError>(
        module: &'ctx WasmDeciderModule,
        command: &'ctx CommandEnvelope,
        stream_id: &'ctx str,
        failure: SnapshotFailure<'ctx, ReadSnapshotError>,
    ) -> WasmSnapshotFailureContext<'ctx, ReadSnapshotError> {
        WasmSnapshotFailureContext {
            module_name: module.name().as_str(),
            module_version: module.version().as_str(),
            command_type: command.type_.as_str(),
            stream_id,
            failure,
        }
    }
}

impl<S, Sched, F> sealed::Sealed for WithSnapshotStore<'_, S, Sched, F> {}

impl<S, Sched, F> WasmExecutionSnapshots for WithSnapshotStore<'_, S, Sched, F>
where
    S: SnapshotRead<OpaqueSnapshotPayload, str>
        + SnapshotWrite<OpaqueSnapshotPayload, str>
        + Clone
        + Send
        + Sync
        + 'static,
    Sched: SnapshotTaskScheduler + Sync,
    F: WasmSnapshotFailurePolicy<<S as SnapshotRead<OpaqueSnapshotPayload, str>>::Error> + Sync,
{
    type ReadError = <S as SnapshotRead<OpaqueSnapshotPayload, str>>::Error;

    /// A snapshot the configured [`WasmSnapshotFailurePolicy`] cannot trust,
    /// whether it failed to read or claims a position ahead of the stream, is
    /// routed through that policy. [`SnapshotFailureDecision::Fail`] returns
    /// the concrete failure, matching this execution's behavior before the
    /// policy existed. [`SnapshotFailureDecision::DiscardAndReplay`] discards
    /// the untrusted snapshot and replays the stream from the beginning
    /// instead, exactly as [`trogon_decider_runtime::CommandExecution`] does
    /// natively.
    async fn load_replay_context<E>(
        &self,
        module: &WasmDeciderModule,
        command: &CommandEnvelope,
        event_store: &E,
        stream_id: &str,
        bounds: ReplayBounds,
    ) -> WasmLoadReplayResult<Self::ReadError, <E as StreamRead<str>>::Error>
    where
        E: StreamRead<str>,
    {
        let snapshot_id = WasmSnapshotId::new(module.name(), module.version(), stream_id);
        let (snapshot_position, mut snapshot_bytes) =
            match <S as SnapshotRead<OpaqueSnapshotPayload, str>>::read_snapshot(
                self.store,
                ReadSnapshotRequest {
                    snapshot_id: snapshot_id.as_str(),
                },
            )
            .await
            {
                Ok(response) => match response.snapshot {
                    Some(snapshot) => (Some(snapshot.position), Some(snapshot.payload.into_bytes())),
                    None => (None, None),
                },
                Err(error) => {
                    let context =
                        Self::snapshot_failure_context(module, command, stream_id, SnapshotFailure::ReadFailed(&error));
                    match self.failure_policy.decide_snapshot_failure(context) {
                        SnapshotFailureDecision::Fail => return Err(WasmLoadReplayError::ReadSnapshot(error)),
                        SnapshotFailureDecision::DiscardAndReplay => (None, None),
                    }
                }
            };

        let from = match snapshot_position {
            Some(position) => ReadFrom::after(position).map_err(WasmLoadReplayError::ReadAfterOverflow)?,
            None => ReadFrom::Beginning,
        };
        let stream_read =
            read_stream_for_execution(event_store, ReadStreamRequest { stream_id, from }, bounds.read_bound(0))
                .await
                .map_err(WasmLoadReplayError::ReadStream)?;
        let mut current_position = stream_read.current_position;
        let mut stream_events = stream_read.events;

        if let Some(position) = snapshot_position
            && let Err(ahead_of_stream) = ensure_snapshot_not_ahead(position, current_position)
        {
            let context = Self::snapshot_failure_context(
                module,
                command,
                stream_id,
                SnapshotFailure::AheadOfStream(ahead_of_stream),
            );
            match self.failure_policy.decide_snapshot_failure(context) {
                SnapshotFailureDecision::Fail => {
                    return Err(WasmLoadReplayError::SnapshotAheadOfStream(ahead_of_stream));
                }
                SnapshotFailureDecision::DiscardAndReplay => {
                    snapshot_bytes = None;

                    let replay = read_stream_for_execution(
                        event_store,
                        ReadStreamRequest {
                            stream_id,
                            from: ReadFrom::Beginning,
                        },
                        bounds.read_bound(0),
                    )
                    .await
                    .map_err(WasmLoadReplayError::ReadStream)?;
                    current_position = replay.current_position;
                    stream_events = replay.events;
                }
            }
        }

        Ok(ReplayContext {
            stream_events,
            current_position,
            snapshot_bytes,
        })
    }

    fn snapshot_cadence(&self, declared: SnapshotCadence) -> SnapshotCadence {
        declared
    }

    fn store(&self, module: &WasmDeciderModule, stream_id: &str, stream_position: StreamPosition, bytes: Vec<u8>) {
        let snapshot_id = WasmSnapshotId::new(module.name(), module.version(), stream_id);
        schedule_snapshot_write(
            self.task_scheduler,
            self.store,
            snapshot_id,
            Snapshot::new(stream_position, OpaqueSnapshotPayload::new(bytes)),
        );
    }
}

impl<E, Sn, G, A, Auth> WasmCommandExecution<'_, E, Sn, G, A, Auth>
where
    E: StreamRead<str> + StreamAppend<str>,
    Sn: WasmExecutionSnapshots + Sync,
    Sn::ReadError: Send + 'static,
    G: NowV7,
    A: CommandAdmission,
    Auth: CommandAuthorizer<CommandEnvelope>,
{
    /// Runs the command against a fresh guest session and appends its decided events.
    ///
    /// What [`with_snapshot_store`](WasmCommandExecution::with_snapshot_store) adds is the snapshot
    /// half: the session resumes from stored bytes so only the events after them are replayed, and
    /// the guest is asked to serialize itself again when the declared
    /// [`SnapshotCadence`] falls due. Without a store the session starts empty and the guest is
    /// never asked to serialize.
    ///
    /// A [`with_admission`](Self::with_admission) limiter, if one is configured, gates all of that:
    /// the permit is taken before the first guest store exists and released when this future
    /// resolves. A [`with_authorizer`](Self::with_authorizer) authorizer, if one is configured, is
    /// consulted immediately after that permit, so a denied command never instantiates the module.
    #[allow(clippy::type_complexity)]
    pub async fn execute(
        self,
    ) -> Result<
        WasmExecutionResult,
        WasmCommandError<Sn::ReadError, <E as StreamRead<str>>::Error, <E as StreamAppend<str>>::Error>,
    > {
        // Held until this execution ends, so the slot covers the guest store's linear memory for
        // as long as that memory is actually allocated rather than only the moment of admission.
        // Held across retries too: a command that re-reads is still one command.
        let _permit = self.admission.admit().map_err(WasmCommandError::Overloaded)?;

        // Before the guest exists and outside the retry loop below: a denied command costs one call
        // and no fuel, and a command that re-reads is still one command answering to one
        // authorization decision.
        self.authorizer
            .authorize_execution(self.principal.as_ref(), self.command)
            .map_err(WasmCommandError::Unauthorized)?;

        let mut retries_left = self.resolved_conflict_retry_budget();
        loop {
            let result = self.attempt().await;

            let WasmCommandError::Append(append_error) = (match result {
                Err(ref error) => error,
                Ok(_) => return result,
            }) else {
                return result;
            };
            if retries_left == 0
                || <E as StreamAppend<str>>::classify_append_failure(self.event_store, append_error)
                    != AppendFailure::WriteConflict
            {
                return result;
            }

            retries_left -= 1;
            metrics().conflict_retries.add(1, &[]);
            tracing::debug!(
                retries_left,
                "another writer advanced the stream; replaying and deciding again"
            );
        }
    }

    /// The replay limit and chunk size this execution was configured with.
    fn replay_bounds(&self) -> ReplayBounds {
        ReplayBounds::new(self.replay_limit, self.replay_chunk_size)
    }

    /// How many retries this execution may spend, once the configured limit is checked against the
    /// preconditions that make a retry meaningful.
    ///
    /// Resolved once rather than per attempt, because none of its inputs can change while the
    /// command runs.
    fn resolved_conflict_retry_budget(&self) -> u32 {
        let declared = command_spec(self.module, &self.command.type_)
            .map_or(WritePrecondition::StreamUnchanged, WasmCommandSpec::write_precondition);
        let retryable = declared == WritePrecondition::StreamUnchanged && self.expected_revision.is_none();
        match self.conflict_retry_limit {
            Some(limit) if retryable => limit.as_u32(),
            _ => 0,
        }
    }

    #[allow(clippy::type_complexity)]
    async fn attempt(
        &self,
    ) -> Result<
        WasmExecutionResult,
        WasmCommandError<Sn::ReadError, <E as StreamRead<str>>::Error, <E as StreamAppend<str>>::Error>,
    > {
        let engine = self.module.engine().clone();
        let decider_pre = self.module.decider_pre().clone();
        let command = self.command.clone();
        let phase_context = GuestPhaseContext::new(self.module, self.command);
        let (mut store, bindings, stream_id) = spawn_guest(move || {
            let mut store = engine.new_store();
            let bindings = instantiate(&mut store, &decider_pre, &engine, &phase_context)?;
            let stream_id = call_stream_id(&mut store, &bindings, &engine, &command)?;
            Ok((store, bindings, stream_id))
        })
        .await?;

        let spec = command_spec(self.module, &self.command.type_);
        let declared = spec.map_or(WritePrecondition::StreamUnchanged, WasmCommandSpec::write_precondition);
        let snapshot_cadence = self.snapshots.snapshot_cadence(
            self.snapshot_cadence
                .unwrap_or_else(|| spec.map_or(SnapshotCadence::Never, WasmCommandSpec::snapshot_cadence)),
        );

        // A command that may only create its stream has no history to load: any history at all
        // would already violate the precondition the append is about to be guarded on.
        let ReplayContext {
            stream_events,
            current_position,
            snapshot_bytes,
        } = if declared == WritePrecondition::NoStream {
            ReplayContext {
                stream_events: Vec::new(),
                current_position: None,
                snapshot_bytes: None,
            }
        } else {
            self.snapshots
                .load_replay_context(
                    self.module,
                    self.command,
                    self.event_store,
                    stream_id.as_str(),
                    self.replay_bounds(),
                )
                .await?
        };

        let mut cursor = ReplayCursor::new(self.replay_bounds(), current_position);
        let mut first_chunk = stream_events;
        cursor.truncate_to_tail(&mut first_chunk);
        ensure_replay_within_limit(self.replay_limit, cursor.advance(&first_chunk))
            .map_err(WasmCommandError::ReplayLimitExceeded)?;

        let precondition = resolve_write_precondition(declared, self.expected_revision, current_position)
            .map_err(WasmCommandError::PreconditionConflict)?;

        let engine = self.module.engine().clone();
        let phase_context = GuestPhaseContext::new(self.module, self.command);
        let first_envelopes = to_any_envelopes(first_chunk);
        let mut guest = spawn_guest(move || {
            let session = create_session(
                &mut store,
                &bindings,
                &engine,
                snapshot_bytes.as_deref(),
                &phase_context,
            )?;
            match replay_events(
                &mut store,
                &bindings,
                &engine,
                session,
                &first_envelopes,
                &phase_context,
            ) {
                Ok(()) => Ok(GuestSession {
                    store,
                    bindings,
                    session,
                }),
                Err(error) => {
                    conclude_session(&mut store, &bindings, &engine, session, &phase_context, Some(&error));
                    Err(error)
                }
            }
        })
        .await?;

        // Everything that can go wrong from here to the end of the walk leaves a live guest session
        // behind, so the loop reports the failure rather than returning through it.
        let replay_failure = loop {
            let Some(next_read) = cursor.next_read() else {
                break None;
            };
            let from = match next_read {
                Ok(from) => from,
                Err(error) => break Some(WasmCommandError::ReadAfterOverflow(error)),
            };
            let read = read_stream_for_execution(
                self.event_store,
                ReadStreamRequest {
                    stream_id: stream_id.as_str(),
                    from,
                },
                cursor.read_bound(),
            )
            .await;
            let mut chunk = match read {
                Ok(response) => response.events,
                Err(error) => break Some(WasmCommandError::ReadStream(error)),
            };
            cursor.truncate_to_tail(&mut chunk);
            if chunk.is_empty() {
                break None;
            }
            if let Err(exceeded) = ensure_replay_within_limit(self.replay_limit, cursor.advance(&chunk)) {
                break Some(WasmCommandError::ReplayLimitExceeded(exceeded));
            }

            guest = fold_chunk(
                guest,
                self.module.engine().clone(),
                GuestPhaseContext::new(self.module, self.command),
                to_any_envelopes(chunk),
            )
            .await?;
        };

        if let Some(failure) = replay_failure {
            discard_session(
                guest,
                self.module.engine().clone(),
                GuestPhaseContext::new(self.module, self.command),
            )
            .await;
            return Err(failure);
        }

        let replayed_event_count = cursor.replayed_event_count();
        let engine = self.module.engine().clone();
        let command = self.command.clone();
        let phase_context = GuestPhaseContext::new(self.module, self.command);
        let (decided_envelopes, new_snapshot_bytes) = spawn_guest(move || {
            let GuestSession {
                mut store,
                bindings,
                session,
            } = guest;
            let outcome = decide(&mut store, &bindings, &engine, session, &command, &phase_context)
                .and_then(ensure_decided_events_are_non_empty)
                .and_then(|decided_envelopes| {
                    // Folding the decided events back into the session is what makes the snapshot
                    // correct (see the module-level ordering note), so it is skipped alongside the
                    // snapshot: on a command that is not due, folding would burn guest fuel producing
                    // state nothing reads.
                    let events_since_snapshot = replayed_event_count.saturating_add(decided_envelopes.len() as u64);
                    if !snapshot_cadence.is_due(events_since_snapshot) {
                        return Ok((decided_envelopes, None));
                    }
                    fold_decided_events(
                        &mut store,
                        &bindings,
                        &engine,
                        session,
                        &decided_envelopes,
                        &phase_context,
                    )?;
                    let new_snapshot_bytes = take_snapshot(&mut store, &bindings, &engine, session, &phase_context)?;
                    Ok((decided_envelopes, new_snapshot_bytes))
                });
            conclude_session(
                &mut store,
                &bindings,
                &engine,
                session,
                &phase_context,
                outcome.as_ref().err(),
            );
            outcome
        })
        .await?;

        let events = encode_events(
            decided_envelopes,
            &self.headers,
            self.command_id,
            &self.event_id_generator,
        );
        let append_response = <E as StreamAppend<str>>::append_stream(
            self.event_store,
            AppendStreamRequest {
                stream_id: stream_id.as_str(),
                stream_write_precondition: precondition,
                events: events.clone(),
            },
        )
        .await
        .map_err(WasmCommandError::Append)?;

        if let Some(bytes) = new_snapshot_bytes {
            self.snapshots
                .store(self.module, stream_id.as_str(), append_response.stream_position, bytes);
        }

        Ok(WasmExecutionResult {
            stream_position: append_response.stream_position,
            events,
        })
    }
}

/// Runs a synchronous, guest-touching closure on a blocking thread pool so it
/// never occupies the async executor for the duration of its fuel and epoch
/// budget. See the module-level doc comment for why this is necessary.
async fn spawn_guest<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    task: impl FnOnce() -> Result<T, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>>
    + Send
    + 'static,
) -> Result<T, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>>
where
    T: Send + 'static,
    ReadSnapshotError: Send + 'static,
    ReadStreamError: Send + 'static,
    AppendStreamError: Send + 'static,
{
    match tokio::task::spawn_blocking(task).await {
        Ok(result) => result,
        Err(join_error) => Err(WasmCommandError::Blocking(join_error)),
    }
}

fn is_epoch_deadline_exceeded(error: &wasmtime::Error) -> bool {
    matches!(error.downcast_ref::<wasmtime::Trap>(), Some(wasmtime::Trap::Interrupt))
}

/// Classifies a wasmtime call failure, surfacing an epoch-deadline interrupt
/// as [`WasmCommandError::DeadlineExceeded`] and delegating every other
/// failure to `fallback`.
fn map_trap<ReadSnapshotError, ReadStreamError, AppendStreamError>(
    error: wasmtime::Error,
    fallback: impl FnOnce(wasmtime::Error) -> WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>,
) -> WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError> {
    if is_epoch_deadline_exceeded(&error) {
        WasmCommandError::DeadlineExceeded(error)
    } else {
        fallback(error)
    }
}

fn instantiate<ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<crate::engine::GuestState>,
    decider_pre: &host::DeciderPre<crate::engine::GuestState>,
    engine: &WasmDeciderEngine,
    context: &GuestPhaseContext,
) -> Result<host::Decider, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    let span = tracing::info_span!(
        span::DECIDER_WASM_INSTANTIATE,
        module_name = %context.module_name,
        module_version = %context.module_version,
        command_type = %context.command_type,
        guest_phase = attribute::GuestPhase::Instantiate.as_str(),
        trap_classification = tracing::field::Empty,
    );
    span.in_scope(|| {
        let fuel_budget = engine.config().fuel_per_call();
        engine
            .arm_guest_call(store, fuel_budget, engine.config().epoch_ticks_per_call())
            .map_err(WasmCommandError::Trap)?;
        let start = Instant::now();
        let result = decider_pre.instantiate(&mut *store);
        let duration = start.elapsed();
        let fuel_consumed = phase_fuel_consumed(store, fuel_budget);
        record_phase_metrics(context, attribute::GuestPhase::Instantiate, duration, fuel_consumed);
        result.map_err(|error| {
            let classification = trap_classification(&error);
            span.record(attribute::TRAP_CLASSIFICATION, classification.as_str());
            record_phase_trap(context, attribute::GuestPhase::Instantiate, classification);
            map_trap(error, WasmCommandError::Instantiate)
        })
    })
}

fn call_stream_id<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &WasmDeciderEngine,
    command: &CommandEnvelope,
) -> Result<String, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    engine
        .arm_guest_call(
            store,
            engine.config().fuel_per_call(),
            engine.config().epoch_ticks_per_call(),
        )
        .map_err(WasmCommandError::Trap)?;
    host::call_stream_id(bindings, store, command)
        .map_err(|error| map_trap(error, WasmCommandError::Trap))?
        .map_err(|error| WasmCommandError::StreamId(error.into()))
}

fn create_session<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &WasmDeciderEngine,
    snapshot: Option<&[u8]>,
    context: &GuestPhaseContext,
) -> Result<host::Session, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    let span = tracing::info_span!(
        span::DECIDER_WASM_INSTANTIATE,
        module_name = %context.module_name,
        module_version = %context.module_version,
        command_type = %context.command_type,
        guest_phase = attribute::GuestPhase::Instantiate.as_str(),
        trap_classification = tracing::field::Empty,
    );
    span.in_scope(|| {
        let fuel_budget = engine.config().fuel_per_call();
        engine
            .arm_guest_call(store, fuel_budget, engine.config().epoch_ticks_per_call())
            .map_err(WasmCommandError::Trap)?;
        let start = Instant::now();
        let result = host::create_session(bindings, store, snapshot);
        let duration = start.elapsed();
        let fuel_consumed = phase_fuel_consumed(store, fuel_budget);
        record_phase_metrics(context, attribute::GuestPhase::Instantiate, duration, fuel_consumed);
        result.map_err(|error| {
            let classification = trap_classification(&error);
            span.record(attribute::TRAP_CLASSIFICATION, classification.as_str());
            record_phase_trap(context, attribute::GuestPhase::Instantiate, classification);
            map_trap(error, WasmCommandError::Trap)
        })
    })
}

fn replay_events<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &WasmDeciderEngine,
    session: host::Session,
    events: &[AnyEnvelope],
    context: &GuestPhaseContext,
) -> Result<(), WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    if events.is_empty() {
        return Ok(());
    }
    let span = tracing::info_span!(
        span::DECIDER_WASM_REPLAY,
        module_name = %context.module_name,
        module_version = %context.module_version,
        command_type = %context.command_type,
        guest_phase = attribute::GuestPhase::Replay.as_str(),
        trap_classification = tracing::field::Empty,
    );
    span.in_scope(|| {
        let fuel_budget = replay_fuel(engine.config().fuel_per_call(), events.len());
        engine
            .arm_guest_call(
                store,
                fuel_budget,
                replay_epoch_ticks(engine.config().epoch_ticks_per_call(), events.len()),
            )
            .map_err(WasmCommandError::Trap)?;
        let start = Instant::now();
        let result = host::evolve(bindings, store, session, events);
        let duration = start.elapsed();
        let fuel_consumed = phase_fuel_consumed(store, fuel_budget);
        record_phase_metrics(context, attribute::GuestPhase::Replay, duration, fuel_consumed);
        match result {
            Ok(inner) => inner.map_err(|error| WasmCommandError::Evolve(error.into())),
            Err(error) => {
                let classification = trap_classification(&error);
                span.record(attribute::TRAP_CLASSIFICATION, classification.as_str());
                record_phase_trap(context, attribute::GuestPhase::Replay, classification);
                Err(map_trap(error, WasmCommandError::Trap))
            }
        }
    })
}

/// Fuel budget for one batched replay `evolve` call.
///
/// The batch grows with stream length while `fuel_per_call` is fixed, so a
/// flat budget would trap legitimate commands on long streams. Scaling
/// linearly keeps fuel a per-event bound, which is the guarantee the sandbox
/// actually needs.
fn replay_fuel(fuel_per_call: u64, event_count: usize) -> u64 {
    u64::try_from(event_count)
        .map(|count| fuel_per_call.saturating_mul(count))
        .unwrap_or(u64::MAX)
}

/// Epoch tick budget for one batched replay `evolve` call, scaled the same
/// way as [`replay_fuel`] so a long replay gets a proportionally larger
/// wall-clock allowance instead of tripping the single-call deadline.
fn replay_epoch_ticks(epoch_ticks_per_call: u64, event_count: usize) -> u64 {
    u64::try_from(event_count)
        .map(|count| epoch_ticks_per_call.saturating_mul(count))
        .unwrap_or(u64::MAX)
}

fn decide<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &WasmDeciderEngine,
    session: host::Session,
    command: &CommandEnvelope,
    context: &GuestPhaseContext,
) -> Result<Vec<AnyEnvelope>, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    let span = tracing::info_span!(
        span::DECIDER_WASM_DECIDE,
        module_name = %context.module_name,
        module_version = %context.module_version,
        command_type = %context.command_type,
        guest_phase = attribute::GuestPhase::Decide.as_str(),
        decision_outcome = tracing::field::Empty,
        trap_classification = tracing::field::Empty,
    );
    span.in_scope(|| {
        let fuel_budget = engine.config().fuel_per_call();
        engine
            .arm_guest_call(store, fuel_budget, engine.config().epoch_ticks_per_call())
            .map_err(WasmCommandError::Trap)?;
        let start = Instant::now();
        let result = host::decide(bindings, store, session, command);
        let duration = start.elapsed();
        let fuel_consumed = phase_fuel_consumed(store, fuel_budget);
        record_phase_metrics(context, attribute::GuestPhase::Decide, duration, fuel_consumed);
        match result {
            Ok(inner) => {
                let decision_outcome = match &inner {
                    Ok(_) => attribute::DecisionOutcome::Decided,
                    Err(DecideError::Rejected(_)) => attribute::DecisionOutcome::Rejected,
                    Err(DecideError::Faulted(_)) => attribute::DecisionOutcome::Faulted,
                };
                span.record(attribute::DECISION_OUTCOME, decision_outcome.as_str());
                inner.map_err(map_decide_error)
            }
            Err(error) => {
                let classification = trap_classification(&error);
                span.record(attribute::TRAP_CLASSIFICATION, classification.as_str());
                record_phase_trap(context, attribute::GuestPhase::Decide, classification);
                Err(map_trap(error, WasmCommandError::Trap))
            }
        }
    })
}

fn map_decide_error<ReadSnapshotError, ReadStreamError, AppendStreamError>(
    error: DecideError,
) -> WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError> {
    match error {
        DecideError::Rejected(detail) => WasmCommandError::Rejected(detail.into()),
        DecideError::Faulted(detail) => WasmCommandError::Faulted(detail.into()),
    }
}

/// Enforces the WIT `decide` contract's non-empty invariant (see `world.wit`)
/// on the host side, rejecting a guest's `Ok([])` before it can reach
/// [`fold_decided_events`] or the stream append.
fn ensure_decided_events_are_non_empty<ReadSnapshotError, ReadStreamError, AppendStreamError>(
    decided_envelopes: Vec<AnyEnvelope>,
) -> Result<Vec<AnyEnvelope>, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    if decided_envelopes.is_empty() {
        Err(WasmCommandError::EmptyDecision)
    } else {
        Ok(decided_envelopes)
    }
}

/// Folds the guest's own newly decided events back into session state before
/// a snapshot can observe them. See the module-level doc comment for why this
/// call is required.
fn fold_decided_events<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &WasmDeciderEngine,
    session: host::Session,
    decided_envelopes: &[AnyEnvelope],
    context: &GuestPhaseContext,
) -> Result<(), WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    replay_events(store, bindings, engine, session, decided_envelopes, context)
}

/// Captures a snapshot of the guest session's current state.
///
/// Must run after [`fold_decided_events`] folds this command's own decided
/// events back into the session; see the module-level doc comment.
fn take_snapshot<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &WasmDeciderEngine,
    session: host::Session,
    context: &GuestPhaseContext,
) -> Result<Option<Vec<u8>>, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    let span = tracing::info_span!(
        span::DECIDER_WASM_SNAPSHOT,
        module_name = %context.module_name,
        module_version = %context.module_version,
        command_type = %context.command_type,
        guest_phase = attribute::GuestPhase::Snapshot.as_str(),
        trap_classification = tracing::field::Empty,
    );
    span.in_scope(|| {
        let fuel_budget = engine.config().fuel_per_call();
        engine
            .arm_guest_call(store, fuel_budget, engine.config().epoch_ticks_per_call())
            .map_err(WasmCommandError::Trap)?;
        let start = Instant::now();
        let result = host::snapshot(bindings, store, session);
        let duration = start.elapsed();
        let fuel_consumed = phase_fuel_consumed(store, fuel_budget);
        record_phase_metrics(context, attribute::GuestPhase::Snapshot, duration, fuel_consumed);
        result.map_err(|error| {
            let classification = trap_classification(&error);
            span.record(attribute::TRAP_CLASSIFICATION, classification.as_str());
            record_phase_trap(context, attribute::GuestPhase::Snapshot, classification);
            map_trap(error, WasmCommandError::Trap)
        })
    })
}

/// A live guest session between the reads of a chunked replay.
///
/// Each chunk is folded by its own guest call on the blocking pool while the reads between them
/// happen on the async executor, so the three handles cross that boundary together, over and over,
/// until the walk reaches the pinned tail.
struct GuestSession {
    store: Store<crate::engine::GuestState>,
    bindings: host::Decider,
    session: host::Session,
}

/// Folds one replay chunk into the session, concluding the session if the fold fails.
///
/// Concluding here rather than at the call site is what keeps the WIT `session` destructor
/// guaranteed: a failed fold has no session left to hand back, so nothing downstream could run it.
async fn fold_chunk<ReadSnapshotError, ReadStreamError, AppendStreamError>(
    guest: GuestSession,
    engine: WasmDeciderEngine,
    context: GuestPhaseContext,
    envelopes: Vec<AnyEnvelope>,
) -> Result<GuestSession, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>>
where
    ReadSnapshotError: Send + 'static,
    ReadStreamError: Send + 'static,
    AppendStreamError: Send + 'static,
{
    spawn_guest(move || {
        let GuestSession {
            mut store,
            bindings,
            session,
        } = guest;
        match replay_events(&mut store, &bindings, &engine, session, &envelopes, &context) {
            Ok(()) => Ok(GuestSession {
                store,
                bindings,
                session,
            }),
            Err(error) => {
                conclude_session(&mut store, &bindings, &engine, session, &context, Some(&error));
                Err(error)
            }
        }
    })
    .await
}

/// Concludes a session abandoned part-way through a chunked replay.
///
/// The guest is blameless here: what failed was a read the host issued between two chunks, and it
/// still has a destructor owed to it. A failure to even reach the blocking pool is discarded for
/// the same reason [`drop_session_discarding_trap`] discards a destructor trap.
async fn discard_session(guest: GuestSession, engine: WasmDeciderEngine, context: GuestPhaseContext) {
    let _ =
        spawn_guest::<(), std::convert::Infallible, std::convert::Infallible, std::convert::Infallible>(move || {
            let GuestSession {
                mut store,
                bindings,
                session,
            } = guest;
            drop_session_discarding_trap(&mut store, &bindings, &engine, session, &context);
            Ok(())
        })
        .await;
}

/// Disposes a guest session once the command's outcome is determined, whether
/// it decided events, rejected, or faulted. The one exception is a guest that
/// already trapped: a trapped component instance cannot be reentered, so
/// attempting the destructor would only re-count the same trap under the
/// `drop` phase.
fn conclude_session<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &WasmDeciderEngine,
    session: host::Session,
    context: &GuestPhaseContext,
    error: Option<&WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>>,
) {
    if matches!(
        error,
        Some(WasmCommandError::Trap(_) | WasmCommandError::DeadlineExceeded(_))
    ) {
        return;
    }
    drop_session_discarding_trap(store, bindings, engine, session, context);
}

/// Drops a guest session after the command outcome it belongs to is already
/// determined: the events [`decide`] returned or the domain error it
/// surfaced, and for a [`WithSnapshotStore`] execution, the state
/// [`fold_decided_events`] folded back in and [`take_snapshot`] serialized.
///
/// Unlike every guest call before it in this flow, the WIT `session`
/// resource's destructor is real guest code (per the `resource session`
/// contract in `world.wit`), so it needs its own fresh fuel and epoch budget
/// here rather than running on whatever `decide` or `snapshot` left behind.
/// Returns the trap instead of propagating it as a [`WasmCommandError`]: by
/// the time this runs, the guest has nothing left to contribute to the
/// command's outcome, so a destructor trap must not discard it.
fn drop_session_after_decide<T>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &WasmDeciderEngine,
    session: host::Session,
) -> wasmtime::Result<()> {
    engine.arm_guest_call(
        store,
        engine.config().fuel_per_call(),
        engine.config().epoch_ticks_per_call(),
    )?;
    host::drop_session(bindings, store, session)
}

/// Runs [`drop_session_after_decide`] and, when it fails, records the trap
/// under [`attribute::GuestPhase::Drop`] and logs it instead of failing the
/// command whose outcome is already decided.
fn drop_session_discarding_trap<T>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &WasmDeciderEngine,
    session: host::Session,
    context: &GuestPhaseContext,
) {
    if let Err(error) = drop_session_after_decide(store, bindings, engine, session) {
        let classification = trap_classification(&error);
        record_phase_trap(context, attribute::GuestPhase::Drop, classification);
        tracing::warn!(
            module_name = %context.module_name,
            module_version = %context.module_version,
            command_type = %context.command_type,
            trap_classification = classification.as_str(),
            error = %error,
            "wasm decider session destructor trapped after its command outcome was already decided; keeping the decided outcome"
        );
    }
}

fn to_any_envelopes(stream_events: Vec<trogon_decider_runtime::StreamEvent>) -> Vec<AnyEnvelope> {
    stream_events
        .into_iter()
        .map(|stream_event| AnyEnvelope {
            type_: stream_event.event.r#type,
            payload: stream_event.event.content,
        })
        .collect()
}

fn encode_events<G>(
    envelopes: Vec<AnyEnvelope>,
    headers: &Headers,
    command_id: Option<CommandId>,
    event_id_generator: &G,
) -> Vec<Event>
where
    G: NowV7,
{
    envelopes
        .into_iter()
        .enumerate()
        .map(|(index, envelope)| Event {
            id: match command_id {
                Some(command_id) => command_id.event_id(index),
                None => EventId::new(event_id_generator.now_v7()),
            },
            r#type: envelope.type_,
            content: envelope.payload,
            headers: headers.clone(),
        })
        .collect()
}

/// Looks up the policies the module descriptor declares for the command being executed.
///
/// A command the descriptor does not declare is not routable to this module, so reaching this
/// point with an unknown type is a host routing bug rather than a domain condition.
fn command_spec<'a>(module: &'a WasmDeciderModule, command_type: &str) -> Option<&'a WasmCommandSpec> {
    module
        .commands()
        .iter()
        .find(|spec| spec.command_type().as_str() == command_type)
}

fn ensure_snapshot_not_ahead(
    snapshot_position: StreamPosition,
    current_position: Option<StreamPosition>,
) -> Result<(), SnapshotAheadOfStream> {
    match current_position {
        Some(stream_position) if snapshot_position <= stream_position => Ok(()),
        stream_position => Err(SnapshotAheadOfStream {
            snapshot_position,
            stream_position,
        }),
    }
}

fn schedule_snapshot_write<S, Sched>(
    task_scheduler: &Sched,
    snapshot_store: &S,
    snapshot_id: WasmSnapshotId,
    snapshot: Snapshot<OpaqueSnapshotPayload>,
) where
    S: SnapshotWrite<OpaqueSnapshotPayload, str> + Clone + Send + Sync + 'static,
    Sched: SnapshotTaskScheduler,
{
    let snapshot_store = snapshot_store.clone();
    let snapshot_id_for_log = snapshot_id.to_string();
    task_scheduler.schedule(async move {
        if let Err(source) = snapshot_store
            .write_snapshot(WriteSnapshotRequest {
                snapshot_id: snapshot_id.as_str(),
                snapshot,
            })
            .await
        {
            tracing::warn!(snapshot_id = %snapshot_id_for_log, error = %source, "failed to write wasm decider snapshot");
        }
    });
}

#[cfg(test)]
mod tests;
