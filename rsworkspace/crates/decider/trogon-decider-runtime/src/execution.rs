//! Runtime boundary for applying decider commands to event streams.
//!
//! Deciders define pure domain behavior: how to identify a stream, rebuild
//! state from events, and decide which new events a command should emit. This
//! module owns the runtime contract around that pure core: load stream history,
//! replay it into state, evaluate the command, encode the decided events, and
//! append them with the correct stream write precondition.
//!
//! Keeping this orchestration here gives storage adapters a narrow job: read
//! and append event envelopes. It also keeps command failures tied to the phase
//! that produced them, so callers can distinguish domain rejection, replay
//! failure, codec failure, and storage failure without losing the concrete
//! source error.

use crate::admission::{CommandAdmission, OverloadedError, WithoutAdmission};
use crate::authorization::{CommandAuthorizer, CommandPrincipal, UnauthorizedError, WithoutAuthorization};
use crate::constants::METER_NAME;
use crate::snapshot::{ReadSnapshotRequest, Snapshot, SnapshotRead, SnapshotType, SnapshotWrite, WriteSnapshotRequest};
use crate::stream::{
    AppendFailure, AppendStreamRequest, AppendStreamResponse, ReadAfterOverflowError, ReadFrom, ReadStreamRequest,
    ReadStreamResponse, StreamAppend, StreamPosition, StreamRead, StreamWritePrecondition,
};
use crate::{
    ConflictRetryLimit, Decider, Event, EventDecode, EventDecodeOutcome, EventEncode, EventId, EventIdentity,
    EventType, Events, Headers, ReplayBounds, ReplayChunkSize, ReplayCursor, ReplayLimit, SnapshotCadence, StreamEvent,
    WritePrecondition,
};
use trogon_decider::{DecisionError, evaluate_decision};
use trogon_semconv::{attribute, metric, span};
use trogon_std::{NowV7, UuidV7Generator};

use crate::CommandId;

use opentelemetry::metrics::Counter;
use opentelemetry::{KeyValue, global};
use tracing::Instrument;

use std::{
    borrow::Borrow,
    future::Future,
    num::NonZeroU64,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

/// Counters recorded for command execution's replay, snapshot read, snapshot
/// write, and conflict retry phases.
struct ExecutionMetrics {
    replay_events: Counter<u64>,
    snapshot_reads: Counter<u64>,
    snapshot_writes: Counter<u64>,
    conflict_retries: Counter<u64>,
}

impl ExecutionMetrics {
    fn new() -> Self {
        let meter = global::meter(METER_NAME);
        Self {
            replay_events: metric::build_decider_replay_events(&meter),
            snapshot_reads: metric::build_decider_snapshot_reads(&meter),
            snapshot_writes: metric::build_decider_snapshot_writes(&meter),
            conflict_retries: metric::build_decider_command_conflict_retries(&meter),
        }
    }
}

static METRICS: OnceLock<ExecutionMetrics> = OnceLock::new();

fn metrics() -> &'static ExecutionMetrics {
    METRICS.get_or_init(ExecutionMetrics::new)
}

fn write_precondition_attribute(precondition: StreamWritePrecondition) -> attribute::WritePrecondition {
    match precondition {
        StreamWritePrecondition::Any => attribute::WritePrecondition::Any,
        StreamWritePrecondition::StreamExists => attribute::WritePrecondition::StreamExists,
        StreamWritePrecondition::NoStream => attribute::WritePrecondition::NoStream,
        StreamWritePrecondition::At(_) => attribute::WritePrecondition::At,
    }
}

/// Resolves the two orthogonal concurrency inputs into the single guard the store checks.
///
/// `declared` is the command's compile-time requirement; `expected_revision` is a client's
/// per-request assertion, present only when the caller read the stream itself. A caller's
/// revision is always read before the host replays, and stream positions are monotonic, so
/// `expected_revision <= observed`: honoring it can cause a rejection but never a wrongly
/// accepted write. [`WritePrecondition::NoStream`] is the one variant it cannot strengthen,
/// because "the stream must not exist" and "the stream must be at revision p" are jointly
/// unsatisfiable rather than merely redundant.
///
/// A revision that exceeds `observed` breaks that ordering rather than merely tightening the
/// guard, so it is rejected as [`RevisionAheadOfStream`] instead of forwarded to the store.
pub fn resolve_write_precondition(
    declared: WritePrecondition,
    expected_revision: Option<StreamPosition>,
    observed: Option<StreamPosition>,
) -> Result<StreamWritePrecondition, PreconditionConflictError> {
    match (declared, expected_revision) {
        (WritePrecondition::StreamUnchanged, None) => Ok(observed.into()),
        (WritePrecondition::NoStream, None) => Ok(StreamWritePrecondition::NoStream),
        (WritePrecondition::StreamExists, None) => Ok(StreamWritePrecondition::StreamExists),
        (WritePrecondition::Any, None) => Ok(StreamWritePrecondition::Any),

        (WritePrecondition::NoStream, Some(_)) => Err(PreconditionConflictError::CreateWithRevision),
        (
            WritePrecondition::StreamUnchanged | WritePrecondition::StreamExists | WritePrecondition::Any,
            Some(expected),
        ) => {
            if observed.is_none_or(|observed| expected > observed) {
                return Err(PreconditionConflictError::RevisionAheadOfStream(
                    RevisionAheadOfStream { expected, observed },
                ));
            }
            Ok(StreamWritePrecondition::At(expected))
        }
    }
}

fn decision_outcome_for_error<D, EV, RS, RD, A, ET, EE, DE>(
    error: &CommandError<D, EV, RS, RD, A, ET, EE, DE>,
) -> attribute::DecisionOutcome {
    match error {
        CommandError::Decide(_) => attribute::DecisionOutcome::Rejected,
        CommandError::Overloaded(_) => attribute::DecisionOutcome::Shed,
        CommandError::Unauthorized(_) => attribute::DecisionOutcome::Denied,
        _ => attribute::DecisionOutcome::Faulted,
    }
}

fn record_snapshot_read_outcome(span: &tracing::Span, outcome: attribute::SnapshotOutcome) {
    span.record(attribute::SNAPSHOT_OUTCOME, outcome.as_str());
    metrics()
        .snapshot_reads
        .add(1, &[KeyValue::new(attribute::SNAPSHOT_OUTCOME, outcome.as_str())]);
}
type CommandEventTypeError<C> = <<C as Decider>::Event as EventType>::Error;
type CommandEventPayloadEncodeError<C> = <<C as Decider>::Event as EventEncode>::Error;
type CommandEventDecodeError<C> = <<C as Decider>::Event as EventDecode>::Error;
type CommandReadStreamError<E, C> = <E as StreamRead<<C as Decider>::StreamId>>::Error;
type CommandAppendStreamError<E, C> = <E as StreamAppend<<C as Decider>::StreamId>>::Error;
type CommandReadSnapshotError<S, C> = <S as SnapshotRead<<C as Decider>::State, <C as Decider>::StreamId>>::Error;

/// Schedules best-effort snapshot writes without erasing the task future type.
///
/// Snapshot execution owns the async block that writes the snapshot. A generic
/// scheduler keeps that future concrete instead of forcing every runtime adapter
/// through a boxed `dyn Future`.
pub trait SnapshotTaskScheduler {
    /// Schedules `task` to run, without blocking the caller on its completion.
    fn schedule<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static;

    /// Waits for every snapshot write this scheduler has accepted to finish.
    ///
    /// The default implementation resolves immediately. Schedulers that defer
    /// writes to work they do not track have nothing to wait for; schedulers
    /// that do track outstanding writes, such as
    /// [`DrainableSnapshotTaskScheduler`], override this to await them.
    fn drain(&self) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }
}

/// Schedules snapshot writes on the ambient Tokio runtime without tracking them.
///
/// This scheduler is fire-and-forget: [`Self::drain`] uses the trait default
/// and resolves immediately, even while writes are still in flight. Hosts that
/// need to await outstanding snapshot writes before teardown should use
/// [`DrainableSnapshotTaskScheduler`] instead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokioSnapshotTaskScheduler;

impl SnapshotTaskScheduler for TokioSnapshotTaskScheduler {
    fn schedule<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::error!("Tokio snapshot task scheduler requires an active Tokio runtime");
            return;
        };

        drop(handle.spawn(task));
    }
}

#[derive(Debug, Default)]
struct SnapshotTaskTracker {
    in_flight: AtomicUsize,
    idle: tokio::sync::Notify,
}

/// Reserves one in-flight slot on construction and releases it on drop.
///
/// Tying the release to `Drop` rather than to normal completion keeps the
/// in-flight count accurate when the wrapped task panics: unwinding still
/// drops this guard, so the counter is decremented and waiters are notified
/// exactly once regardless of how the task ends.
#[derive(Debug)]
struct SnapshotTaskGuard {
    tasks: Arc<SnapshotTaskTracker>,
}

impl SnapshotTaskGuard {
    fn new(tasks: Arc<SnapshotTaskTracker>) -> Self {
        tasks.in_flight.fetch_add(1, Ordering::SeqCst);
        Self { tasks }
    }
}

impl Drop for SnapshotTaskGuard {
    fn drop(&mut self) {
        if self.tasks.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.tasks.idle.notify_waiters();
        }
    }
}

/// Schedules snapshot writes on the ambient Tokio runtime and tracks them so
/// hosts can await outstanding writes before teardown.
///
/// Every scheduled task increments an in-flight counter and decrements it on
/// completion. [`Self::drain`] waits until that counter reaches zero. Clone
/// this scheduler to share the same in-flight tracking between the executions
/// that schedule writes and the host that drains them; cloning is cheap, it
/// only bumps a reference count.
#[derive(Debug, Clone, Default)]
pub struct DrainableSnapshotTaskScheduler {
    tasks: Arc<SnapshotTaskTracker>,
}

impl DrainableSnapshotTaskScheduler {
    /// Creates a scheduler with no in-flight snapshot writes.
    pub fn new() -> Self {
        Self::default()
    }
}

impl SnapshotTaskScheduler for DrainableSnapshotTaskScheduler {
    fn schedule<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::error!("Tokio snapshot task scheduler requires an active Tokio runtime");
            return;
        };

        let guard = SnapshotTaskGuard::new(Arc::clone(&self.tasks));
        drop(handle.spawn(async move {
            let _guard = guard;
            task.await;
        }));
    }

    fn drain(&self) -> impl Future<Output = ()> + Send {
        let tasks = Arc::clone(&self.tasks);
        async move {
            loop {
                let idle = tasks.idle.notified();
                if tasks.in_flight.load(Ordering::SeqCst) == 0 {
                    break;
                }
                idle.await;
            }
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
/// Runs snapshot tasks to completion before returning.
///
/// This scheduler is test support. It runs the task on a helper thread so sync
/// tests can call `block_on(command.execute())` without entering the futures
/// executor recursively. Tokio-backed stores should use
/// `TokioSnapshotTaskScheduler` so their async I/O runs inside the runtime they
/// require.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImmediateSnapshotTaskScheduler;

#[cfg(any(test, feature = "test-support"))]
impl SnapshotTaskScheduler for ImmediateSnapshotTaskScheduler {
    fn schedule<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let handle = std::thread::spawn(move || futures::executor::block_on(task));
        if handle.join().is_err() {
            tracing::warn!("test snapshot task panicked");
        }
    }
}

/// Outcome of consulting a [`SnapshotPolicy`] after a successful command execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotDecision {
    /// Do not take a snapshot for this execution.
    Skip,
    /// Take a snapshot for this execution.
    Take,
}

/// Context passed to a [`SnapshotPolicy`] so it can decide whether to snapshot.
#[derive(Debug, Clone, Copy)]
pub struct DecideSnapshot<'a, C: Decider> {
    /// The command that produced the execution result.
    pub command: &'a C,
    /// The stream high-watermark after the append that may trigger a snapshot.
    ///
    /// Use this as the position for a new snapshot if the policy decides to snapshot.
    /// Do not use it as a gapless event count.
    pub stream_position: StreamPosition,
    /// Snapshot position before this execution replayed trailing events.
    ///
    /// `None` means execution started without a snapshot.
    pub snapshot_position: Option<StreamPosition>,
    /// State after replaying history and applying the newly decided events.
    pub state: &'a C::State,
    /// Events decided by this execution, already appended to the stream.
    pub events: &'a Events<C::Event>,
    /// Number of persisted stream events read after the snapshot position.
    pub replayed_event_count: u64,
}

/// Decides whether a command execution should take a snapshot after it appends events.
pub trait SnapshotPolicy<C: Decider> {
    /// Decides whether to take a snapshot for the given execution context.
    fn decide_snapshot(&self, context: DecideSnapshot<'_, C>) -> SnapshotDecision;
}

/// Associates a [`Decider`] with the [`SnapshotPolicy`] its command executions
/// should use, so callers can build a configured [`Snapshots`] without
/// repeating the policy at every call site.
pub trait CommandSnapshotPolicy: Decider
where
    Self::State: SnapshotType,
{
    /// The snapshot policy this decider's command executions use.
    type SnapshotPolicy: SnapshotPolicy<Self>;

    /// The snapshot policy instance this decider's command executions use.
    const SNAPSHOT_POLICY: Self::SnapshotPolicy;

    /// Builds the [`Snapshots`] configuration for this decider from a snapshot store.
    fn snapshots<'a, S>(snapshot_store: &'a S) -> Snapshots<'a, S, Self::SnapshotPolicy> {
        Snapshots::new(snapshot_store, Self::SNAPSHOT_POLICY)
    }
}

/// Applies the cadence a decider declares through
/// [`Decider::SNAPSHOT_CADENCE`](trogon_decider::Decider::SNAPSHOT_CADENCE).
///
/// Prefer this over [`NoSnapshot`] or [`FrequencySnapshot`] when a decider also runs on the
/// sandboxed path: the WASM host reads the same declaration out of the module descriptor, so
/// selecting it here is what keeps the two paths on one cadence.
///
/// ```ignore
/// impl CommandSnapshotPolicy for PauseSchedule {
///     type SnapshotPolicy = SnapshotCadence;
///     const SNAPSHOT_POLICY: SnapshotCadence = Self::SNAPSHOT_CADENCE;
/// }
/// ```
impl<C: Decider> SnapshotPolicy<C> for SnapshotCadence {
    fn decide_snapshot(&self, context: DecideSnapshot<'_, C>) -> SnapshotDecision {
        if self.is_due(context.replayed_event_count + context.events.len() as u64) {
            SnapshotDecision::Take
        } else {
            SnapshotDecision::Skip
        }
    }
}

/// A [`SnapshotPolicy`] that never takes a snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoSnapshot;

impl<C: Decider> SnapshotPolicy<C> for NoSnapshot {
    fn decide_snapshot(&self, _context: DecideSnapshot<'_, C>) -> SnapshotDecision {
        SnapshotDecision::Skip
    }
}

/// A [`SnapshotPolicy`] that takes a snapshot once at least `frequency` events
/// have been read or appended since the last snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrequencySnapshot {
    frequency: NonZeroU64,
}

impl FrequencySnapshot {
    /// Creates a policy that snapshots every `frequency` events.
    pub const fn new(frequency: NonZeroU64) -> Self {
        Self { frequency }
    }

    /// Returns the configured snapshot frequency.
    pub const fn frequency(self) -> NonZeroU64 {
        self.frequency
    }
}

impl<C: Decider> SnapshotPolicy<C> for FrequencySnapshot {
    fn decide_snapshot(&self, context: DecideSnapshot<'_, C>) -> SnapshotDecision {
        if context.replayed_event_count + context.events.len() as u64 >= self.frequency.get() {
            SnapshotDecision::Take
        } else {
            SnapshotDecision::Skip
        }
    }
}

/// Successful outcome of one command execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionResult<State, Event> {
    /// The stream high-watermark after the command append completed.
    pub stream_position: StreamPosition,
    /// Domain events emitted by the command after successful append.
    pub events: Events<Event>,
    /// State after replaying history and applying the emitted events.
    pub state: State,
}

/// Result returned by command execution.
///
/// Command execution is the first layer that knows which phase failed, so this
/// type keeps phase information here instead of forcing storage traits to wrap
/// their own errors. The operation errors stay concrete and separate to preserve
/// compiler diagnostics and avoid boxing or a lossy shared infrastructure enum.
pub type CommandResult<C, ReadSnapshotError, ReadStreamError, AppendStreamError> = Result<
    ExecutionResult<<C as Decider>::State, <C as Decider>::Event>,
    CommandError<
        <C as Decider>::DecideError,
        <C as Decider>::EvolveError,
        ReadSnapshotError,
        ReadStreamError,
        AppendStreamError,
        CommandEventTypeError<C>,
        CommandEventPayloadEncodeError<C>,
        CommandEventDecodeError<C>,
    >,
>;

/// Error taxonomy for a command execution attempt.
///
/// The command boundary normalizes failures by execution phase while preserving
/// the exact source error type for each phase. Domain failures come from the
/// decider, storage failures come from the concrete read/append/snapshot
/// operation that failed, and codec failures stay tied to the event traits.
#[derive(Debug, thiserror::Error)]
pub enum CommandError<
    DecideError,
    EvolveError,
    ReadSnapshotError,
    ReadStreamError,
    AppendStreamError,
    EventTypeError,
    PayloadEncodeError,
    DecodeError,
> {
    /// The command could not decide because the domain rejected it.
    #[error("command decision failed: {0}")]
    Decide(#[source] DecideError),
    /// The command or replay could not evolve state from an event.
    #[error("command state evolution failed: {0}")]
    Evolve(#[source] EvolveError),
    /// Snapshot loading failed before replaying stream history.
    #[error("command snapshot read failed: {0}")]
    ReadSnapshot(#[source] ReadSnapshotError),
    /// Stream history loading failed.
    #[error("command stream read failed: {0}")]
    ReadStream(#[source] ReadStreamError),
    /// Appending the decided events failed after the command was accepted.
    #[error("command stream append failed: {0}")]
    Append(#[source] AppendStreamError),
    /// A decided domain event could not provide its stored event type.
    #[error("command event type failed: {0}")]
    EventType(#[source] EventTypeError),
    /// A decided domain event could not encode its payload.
    #[error("command event encoding failed: {0}")]
    EventEncode(#[source] PayloadEncodeError),
    /// A stored event could not be converted back into a domain event.
    #[error("command event decoding failed: {0}")]
    DecodeEvent(#[source] DecodeError),
    /// The loaded snapshot claims a position newer than the stream can prove.
    #[error("{0}")]
    SnapshotAheadOfStream(SnapshotAheadOfStream),
    /// The snapshot's recorded position cannot be advanced (u64 overflow).
    #[error("{0}")]
    ReadAfterOverflow(#[source] ReadAfterOverflowError),
    /// The stream read for this command returned more events than the
    /// configured [`ReplayLimit`] allows to replay.
    #[error("{0}")]
    ReplayLimitExceeded(ReplayLimitExceeded),
    /// The caller's expected revision cannot be reconciled with the command's
    /// declared [`WritePrecondition`].
    #[error("{0}")]
    PreconditionConflict(#[source] PreconditionConflictError),
    /// The configured [`CommandAdmission`] limiter had no capacity, so the
    /// command was shed before any work began.
    #[error("{0}")]
    Overloaded(OverloadedError),
    /// The configured [`CommandAuthorizer`] refused this principal, or none was
    /// supplied, so the command was denied before any work began.
    #[error("{0}")]
    Unauthorized(#[source] UnauthorizedError),
}

/// A caller-supplied expected revision that no stream state can satisfy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PreconditionConflictError {
    /// The command declares [`WritePrecondition::NoStream`], so it may only
    /// create a stream, while the caller asserted the stream had already
    /// reached a specific revision.
    ///
    /// Raised before the stream is read: the two assertions are jointly
    /// unsatisfiable whatever the stream turns out to hold.
    #[error("command requires an empty stream but the caller supplied an expected revision")]
    CreateWithRevision,
    /// The caller asserted a revision the stream has never reached.
    #[error("{0}")]
    RevisionAheadOfStream(RevisionAheadOfStream),
}

/// Error detail for a caller-supplied expected revision that exceeds the
/// stream position this execution observed.
///
/// Every store behind this crate serves reads from the stream leader, so a
/// caller's revision is read from the same authority this execution replays
/// from, and stream positions only advance. A caller who read before this
/// execution replayed therefore cannot legitimately hold a revision the
/// replay did not see: such a revision was never assigned to any event.
///
/// It is rejected rather than passed through as
/// [`StreamWritePrecondition::At`] because the append would fail its guard
/// anyway, and it would fail the way ordinary contention does. A caller
/// retrying a fabricated revision would loop on a conflict that no retry can
/// resolve, so naming it here turns a permanent failure back into one.
///
/// A store that served replayed reads from a lagging replica would break the
/// leader-read premise and make this reachable for a correct caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevisionAheadOfStream {
    /// Revision the caller asserted the stream had reached.
    pub expected: StreamPosition,
    /// Stream high-watermark this execution observed, or `None` if the stream
    /// does not exist yet.
    pub observed: Option<StreamPosition>,
}

impl std::fmt::Display for RevisionAheadOfStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.observed {
            Some(observed) => write!(
                f,
                "caller expected revision {} but the stream has only reached {observed}",
                self.expected
            ),
            None => write!(
                f,
                "caller expected revision {} but the stream does not exist",
                self.expected
            ),
        }
    }
}

/// Error detail for a loaded snapshot whose recorded position the stream
/// cannot prove happened, for example after the stream was truncated or
/// replaced without clearing its snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotAheadOfStream {
    /// Position recorded on the loaded snapshot.
    pub snapshot_position: StreamPosition,
    /// Stream high-watermark observed when the snapshot was checked, or
    /// `None` if the stream has no current position.
    pub stream_position: Option<StreamPosition>,
}

impl std::fmt::Display for SnapshotAheadOfStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.stream_position {
            Some(stream_position) => write!(
                f,
                "snapshot position {} is ahead of current stream position {stream_position}",
                self.snapshot_position
            ),
            None => write!(
                f,
                "snapshot position {} exists but the stream has no current position",
                self.snapshot_position
            ),
        }
    }
}

/// Error detail for a command execution whose stream read returned more
/// events than its configured [`ReplayLimit`] allowed to replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayLimitExceeded {
    /// The replay limit configured for this execution.
    pub limit: ReplayLimit,
    /// The number of stream events the read returned.
    ///
    /// The read is bounded to `limit + 1` events once a [`ReplayLimit`] is
    /// configured, so this is `limit + 1` rather than the stream's true
    /// length whenever the true length is greater than that.
    pub replayed_event_count: u64,
}

impl std::fmt::Display for ReplayLimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "command replay read {} events, exceeding the configured limit of {}",
            self.replayed_event_count, self.limit
        )
    }
}

/// A recoverable snapshot failure, kept distinct from [`CommandError`] so a
/// [`SnapshotFailurePolicy`] can inspect the failed operation before deciding
/// whether the command should fail or recover.
#[derive(Debug)]
pub enum SnapshotFailure<'a, ReadSnapshotError> {
    /// The loaded snapshot claims a position newer than the stream can prove.
    AheadOfStream(SnapshotAheadOfStream),
    /// Reading the snapshot failed, which also covers a snapshot payload that
    /// failed to decode, since the read adapter folds decode failures into
    /// its own error type.
    ReadFailed(&'a ReadSnapshotError),
}

/// Context passed to a [`SnapshotFailurePolicy`] when a snapshot failure occurs.
#[derive(Debug)]
pub struct SnapshotFailureContext<'a, C: Decider, ReadSnapshotError> {
    /// The command that triggered the failing execution.
    pub command: &'a C,
    /// The failure the policy must decide how to handle.
    pub failure: SnapshotFailure<'a, ReadSnapshotError>,
}

/// The outcome a [`SnapshotFailurePolicy`] chooses for a [`SnapshotFailure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotFailureDecision {
    /// Fail the command with the concrete [`CommandError`] for this failure.
    Fail,
    /// Discard the bad snapshot and replay the command from the beginning of
    /// the stream, as if no snapshot had ever been taken.
    DiscardAndReplay,
}

/// Chooses how a [`CommandExecution`] reacts to a snapshot it cannot trust.
///
/// This mirrors [`SnapshotPolicy`]: it lets callers plug in per-command or
/// per-store logic instead of hard-coding one reaction for every decider.
/// [`FailOnSnapshotFailure`] keeps today's behavior of failing the command;
/// [`DiscardAndReplaySnapshotFailure`] discards the bad snapshot and replays
/// from the beginning of the stream. Custom implementations can choose
/// per [`SnapshotFailure`] kind.
pub trait SnapshotFailurePolicy<C: Decider, ReadSnapshotError> {
    /// Decides how the command execution should react to the given snapshot failure.
    fn decide_snapshot_failure(
        &self,
        context: SnapshotFailureContext<'_, C, ReadSnapshotError>,
    ) -> SnapshotFailureDecision;
}

/// Fails the command on any snapshot failure. This is the default policy and
/// matches the runtime's behavior before [`SnapshotFailurePolicy`] existed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FailOnSnapshotFailure;

impl<C: Decider, ReadSnapshotError> SnapshotFailurePolicy<C, ReadSnapshotError> for FailOnSnapshotFailure {
    fn decide_snapshot_failure(
        &self,
        _context: SnapshotFailureContext<'_, C, ReadSnapshotError>,
    ) -> SnapshotFailureDecision {
        SnapshotFailureDecision::Fail
    }
}

/// Discards a bad snapshot and replays the command from the beginning of the
/// stream instead of failing it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiscardAndReplaySnapshotFailure;

impl<C: Decider, ReadSnapshotError> SnapshotFailurePolicy<C, ReadSnapshotError> for DiscardAndReplaySnapshotFailure {
    fn decide_snapshot_failure(
        &self,
        _context: SnapshotFailureContext<'_, C, ReadSnapshotError>,
    ) -> SnapshotFailureDecision {
        SnapshotFailureDecision::DiscardAndReplay
    }
}

#[derive(Debug, thiserror::Error)]
enum ReplayStreamError<EvolveError, DecodeError> {
    #[error("{0}")]
    Evolve(#[source] EvolveError),
    #[error("{0}")]
    DecodeEvent(#[source] DecodeError),
}

#[derive(Debug, thiserror::Error)]
enum AppendDecisionError<DecideError, EvolveError, AppendStreamError, EventTypeError, PayloadEncodeError> {
    #[error("{0}")]
    Decide(#[source] DecideError),
    #[error("{0}")]
    Evolve(#[source] EvolveError),
    #[error("{0}")]
    Append(#[source] AppendStreamError),
    #[error("{0}")]
    EventType(#[source] EventTypeError),
    #[error("{0}")]
    EventEncode(#[source] PayloadEncodeError),
    #[error("{0}")]
    PreconditionConflict(#[source] PreconditionConflictError),
}

impl<
    DecideError,
    EvolveError,
    ReadSnapshotError,
    ReadStreamError,
    AppendStreamError,
    EventTypeError,
    PayloadEncodeError,
    DecodeError,
> From<ReplayStreamError<EvolveError, DecodeError>>
    for CommandError<
        DecideError,
        EvolveError,
        ReadSnapshotError,
        ReadStreamError,
        AppendStreamError,
        EventTypeError,
        PayloadEncodeError,
        DecodeError,
    >
{
    fn from(error: ReplayStreamError<EvolveError, DecodeError>) -> Self {
        match error {
            ReplayStreamError::Evolve(error) => Self::Evolve(error),
            ReplayStreamError::DecodeEvent(error) => Self::DecodeEvent(error),
        }
    }
}

impl<
    DecideError,
    EvolveError,
    ReadSnapshotError,
    ReadStreamError,
    AppendStreamError,
    EventTypeError,
    PayloadEncodeError,
    DecodeError,
> From<AppendDecisionError<DecideError, EvolveError, AppendStreamError, EventTypeError, PayloadEncodeError>>
    for CommandError<
        DecideError,
        EvolveError,
        ReadSnapshotError,
        ReadStreamError,
        AppendStreamError,
        EventTypeError,
        PayloadEncodeError,
        DecodeError,
    >
{
    fn from(
        error: AppendDecisionError<DecideError, EvolveError, AppendStreamError, EventTypeError, PayloadEncodeError>,
    ) -> Self {
        match error {
            AppendDecisionError::Decide(error) => Self::Decide(error),
            AppendDecisionError::Evolve(error) => Self::Evolve(error),
            AppendDecisionError::Append(error) => Self::Append(error),
            AppendDecisionError::EventType(error) => Self::EventType(error),
            AppendDecisionError::EventEncode(error) => Self::EventEncode(error),
            AppendDecisionError::PreconditionConflict(error) => Self::PreconditionConflict(error),
        }
    }
}

mod sealed;

/// State a command execution resumes from, plus the stream events recorded after it.
pub struct ReplayContext<State> {
    /// State to fold `stream_events` onto: a snapshot's payload, or the decider's initial state.
    state: State,
    /// Position `state` was snapshotted at, or `None` when replay starts from the beginning.
    snapshot_position: Option<StreamPosition>,
    stream_events: Vec<StreamEvent>,
    /// Stream high-watermark observed by the read that produced `stream_events`.
    current_position: Option<StreamPosition>,
    /// A stored snapshot could not be trusted and was thrown away, so this execution owes the
    /// store a replacement whatever the [`SnapshotPolicy`] would otherwise decide.
    discarded_snapshot: bool,
}

/// What [`ExecutionSnapshots::load_replay_context`] hands back to the execution skeleton.
pub type LoadReplayResult<State, ReadSnapshotError, ReadStreamError> =
    Result<ReplayContext<State>, LoadReplayError<ReadSnapshotError, ReadStreamError>>;

/// Why a command execution could not assemble the state and history to replay.
///
/// Widened into the matching [`CommandError`] variants by the skeleton that called it.
#[derive(Debug, thiserror::Error)]
pub enum LoadReplayError<ReadSnapshotError, ReadStreamError> {
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

impl<
    DecideError,
    EvolveError,
    ReadSnapshotError,
    ReadStreamError,
    AppendStreamError,
    EventTypeError,
    PayloadEncodeError,
    DecodeError,
> From<LoadReplayError<ReadSnapshotError, ReadStreamError>>
    for CommandError<
        DecideError,
        EvolveError,
        ReadSnapshotError,
        ReadStreamError,
        AppendStreamError,
        EventTypeError,
        PayloadEncodeError,
        DecodeError,
    >
{
    fn from(error: LoadReplayError<ReadSnapshotError, ReadStreamError>) -> Self {
        match error {
            LoadReplayError::ReadSnapshot(error) => Self::ReadSnapshot(error),
            LoadReplayError::ReadStream(error) => Self::ReadStream(error),
            LoadReplayError::SnapshotAheadOfStream(ahead) => Self::SnapshotAheadOfStream(ahead),
            LoadReplayError::ReadAfterOverflow(error) => Self::ReadAfterOverflow(error),
        }
    }
}

/// The snapshot half of a command execution.
///
/// [`CommandExecution::execute`] runs one skeleton for every configuration: resolve the stream id,
/// load what to replay, fold it, decide, append, persist. This trait is the only part of that
/// skeleton that varies, so a change to the shared phases (a retry loop, an admission gate, an
/// authorization step) is a change in one place rather than one per configuration.
///
/// It stays a trait rather than an `Option` because the two configurations do not accept the same
/// deciders. Snapshotting needs `C::State` to be clonable, sendable, and nameable as a snapshot
/// type; a decider that never snapshots owes none of that. Those bounds therefore live on the
/// [`Snapshots`] implementation, and the skeleton inherits whichever set applies.
pub trait ExecutionSnapshots<C>: sealed::Sealed
where
    C: Decider,
{
    /// How a snapshot read can fail. [`Infallible`](std::convert::Infallible) when there is no
    /// store to read from.
    type ReadError;

    /// Loads the state to replay onto and the stream events recorded after it.
    ///
    /// Owning the stream read (rather than being handed its result) is what lets a configuration
    /// read from a snapshot position and, when that snapshot turns out to be untrustworthy, read
    /// the stream again from the beginning.
    fn load_replay_context<E>(
        &self,
        event_store: &E,
        command: &C,
        stream_id: &C::StreamId,
        bounds: ReplayBounds,
    ) -> impl Future<Output = LoadReplayResult<C::State, Self::ReadError, CommandReadStreamError<E, C>>> + Send
    where
        E: StreamRead<C::StreamId>;

    /// Persists the state this execution ended at, if the configuration keeps any.
    ///
    /// `discarded_snapshot` reports that [`load_replay_context`](Self::load_replay_context) threw
    /// away a snapshot it could not trust.
    fn store(&self, stream_id: &C::StreamId, discarded_snapshot: bool, context: DecideSnapshot<'_, C>);
}

/// Marks a [`CommandExecution`] that has not been configured with a
/// snapshot store, so it always replays from the beginning of the stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WithoutSnapshots;

impl sealed::Sealed for WithoutSnapshots {}

impl<C> ExecutionSnapshots<C> for WithoutSnapshots
where
    C: Decider + Sync,
    C::State: Send,
    C::StreamId: Sync,
{
    type ReadError = std::convert::Infallible;

    async fn load_replay_context<E>(
        &self,
        event_store: &E,
        _command: &C,
        stream_id: &C::StreamId,
        bounds: ReplayBounds,
    ) -> LoadReplayResult<C::State, Self::ReadError, CommandReadStreamError<E, C>>
    where
        E: StreamRead<C::StreamId>,
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
        .map_err(LoadReplayError::ReadStream)?;

        Ok(ReplayContext {
            state: C::initial_state(),
            snapshot_position: None,
            current_position: stream_read.current_position,
            stream_events: stream_read.events,
            discarded_snapshot: false,
        })
    }

    fn store(&self, _stream_id: &C::StreamId, _discarded_snapshot: bool, _context: DecideSnapshot<'_, C>) {}
}

/// Marks a [`Snapshots`] configuration that has not been given a
/// [`SnapshotTaskScheduler`], so snapshot writes are not scheduled at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WithoutSnapshotTaskScheduler;

/// Snapshot store, policy, task scheduler, and failure policy configured for
/// a [`CommandExecution`].
pub struct Snapshots<'a, S, P, Spawn = WithoutSnapshotTaskScheduler, F = FailOnSnapshotFailure> {
    snapshot_store: &'a S,
    policy: P,
    schedule_snapshot_task: Spawn,
    failure_policy: F,
}

impl<'a, S, P> Snapshots<'a, S, P, WithoutSnapshotTaskScheduler, FailOnSnapshotFailure> {
    /// Configures a snapshot store and policy with no task scheduler and the
    /// default [`FailOnSnapshotFailure`] failure policy.
    pub fn new(snapshot_store: &'a S, policy: P) -> Self {
        Self {
            snapshot_store,
            policy,
            schedule_snapshot_task: WithoutSnapshotTaskScheduler,
            failure_policy: FailOnSnapshotFailure,
        }
    }
}

impl<'a, S, P, Spawn, F> Snapshots<'a, S, P, Spawn, F> {
    fn schedule_snapshot_tasks_with<NextSpawn>(
        self,
        schedule_snapshot_task: NextSpawn,
    ) -> Snapshots<'a, S, P, NextSpawn, F> {
        Snapshots {
            snapshot_store: self.snapshot_store,
            policy: self.policy,
            schedule_snapshot_task,
            failure_policy: self.failure_policy,
        }
    }

    fn with_snapshot_failure_policy<NextF>(self, failure_policy: NextF) -> Snapshots<'a, S, P, Spawn, NextF> {
        Snapshots {
            snapshot_store: self.snapshot_store,
            policy: self.policy,
            schedule_snapshot_task: self.schedule_snapshot_task,
            failure_policy,
        }
    }
}

impl<S, P, Spawn, F> sealed::Sealed for Snapshots<'_, S, P, Spawn, F> {}

impl<C, S, P, Spawn, F> ExecutionSnapshots<C> for Snapshots<'_, S, P, Spawn, F>
where
    C: Decider + Sync,
    C::State: Clone + SnapshotType + Send + 'static,
    C::StreamId: std::fmt::Display + Sync + ToOwned,
    <C::StreamId as ToOwned>::Owned: Borrow<C::StreamId> + Send + 'static,
    S: Clone + SnapshotRead<C::State, C::StreamId> + SnapshotWrite<C::State, C::StreamId> + 'static,
    P: SnapshotPolicy<C> + Sync,
    Spawn: SnapshotTaskScheduler + Send + Sync,
    F: SnapshotFailurePolicy<C, CommandReadSnapshotError<S, C>> + Sync,
{
    type ReadError = CommandReadSnapshotError<S, C>;

    async fn load_replay_context<E>(
        &self,
        event_store: &E,
        command: &C,
        stream_id: &C::StreamId,
        bounds: ReplayBounds,
    ) -> LoadReplayResult<C::State, Self::ReadError, CommandReadStreamError<E, C>>
    where
        E: StreamRead<C::StreamId>,
    {
        let read_snapshot_span = tracing::info_span!(
            span::DECIDER_READ_SNAPSHOT,
            stream_id = %stream_id,
            snapshot_outcome = tracing::field::Empty,
        );

        let mut discarded_snapshot = false;
        let (mut snapshot_position, mut state, mut snapshot_outcome) = match self
            .snapshot_store
            .read_snapshot(ReadSnapshotRequest { snapshot_id: stream_id })
            .instrument(read_snapshot_span.clone())
            .await
        {
            Ok(response) => {
                let snapshot = response.snapshot;
                let snapshot_position = snapshot.as_ref().map(|snapshot| snapshot.position);
                let outcome = if snapshot_position.is_some() {
                    attribute::SnapshotOutcome::Hit
                } else {
                    attribute::SnapshotOutcome::Miss
                };
                let state = snapshot
                    .map(|snapshot| snapshot.payload)
                    .unwrap_or_else(C::initial_state);
                (snapshot_position, state, outcome)
            }
            Err(error) => {
                let decision = self.failure_policy.decide_snapshot_failure(SnapshotFailureContext {
                    command,
                    failure: SnapshotFailure::ReadFailed(&error),
                });
                match decision {
                    SnapshotFailureDecision::Fail => {
                        record_snapshot_read_outcome(&read_snapshot_span, attribute::SnapshotOutcome::Failed);
                        return Err(LoadReplayError::ReadSnapshot(error));
                    }
                    SnapshotFailureDecision::DiscardAndReplay => {
                        discarded_snapshot = true;
                        (
                            None,
                            C::initial_state(),
                            attribute::SnapshotOutcome::DiscardedReadFailure,
                        )
                    }
                }
            }
        };

        let from = match snapshot_position {
            Some(position) => ReadFrom::after(position).map_err(LoadReplayError::ReadAfterOverflow)?,
            None => ReadFrom::Beginning,
        };
        let stream_read =
            read_stream_for_execution(event_store, ReadStreamRequest { stream_id, from }, bounds.read_bound(0))
                .await
                .map_err(LoadReplayError::ReadStream)?;
        let mut current_position = stream_read.current_position;
        let mut stream_events = stream_read.events;

        if let Some(position) = snapshot_position
            && let Err(ahead_of_stream) = ensure_snapshot_not_ahead(position, current_position)
        {
            let decision = self.failure_policy.decide_snapshot_failure(SnapshotFailureContext {
                command,
                failure: SnapshotFailure::AheadOfStream(ahead_of_stream),
            });
            match decision {
                SnapshotFailureDecision::Fail => {
                    record_snapshot_read_outcome(&read_snapshot_span, attribute::SnapshotOutcome::Failed);
                    return Err(LoadReplayError::SnapshotAheadOfStream(ahead_of_stream));
                }
                SnapshotFailureDecision::DiscardAndReplay => {
                    discarded_snapshot = true;
                    snapshot_position = None;
                    state = C::initial_state();
                    snapshot_outcome = attribute::SnapshotOutcome::DiscardedAheadOfStream;

                    let replay = read_stream_for_execution(
                        event_store,
                        ReadStreamRequest {
                            stream_id,
                            from: ReadFrom::Beginning,
                        },
                        bounds.read_bound(0),
                    )
                    .await
                    .map_err(LoadReplayError::ReadStream)?;
                    current_position = replay.current_position;
                    stream_events = replay.events;
                }
            }
        }

        record_snapshot_read_outcome(&read_snapshot_span, snapshot_outcome);

        Ok(ReplayContext {
            state,
            snapshot_position,
            stream_events,
            current_position,
            discarded_snapshot,
        })
    }

    fn store(&self, stream_id: &C::StreamId, discarded_snapshot: bool, context: DecideSnapshot<'_, C>) {
        if discarded_snapshot {
            // The discarded snapshot is still sitting in the store at this
            // stream id. The normal policy might choose to skip a snapshot
            // this time, which would leave that stale or undecodable payload
            // in place for the next execution to trip over again. Writing
            // unconditionally here overwrites it with a snapshot the recovered
            // execution can trust.
            schedule_snapshot_write(
                &self.schedule_snapshot_task,
                self.snapshot_store,
                stream_id,
                Snapshot::new(context.stream_position, context.state.clone()),
            );
        } else {
            maybe_take_snapshot(self, stream_id, context);
        }
    }
}

/// Converts a value into a [`Snapshots`] configuration for a [`Decider`].
///
/// Implemented for a plain snapshot store reference (using the decider's
/// [`CommandSnapshotPolicy`]) and for an already-built [`Snapshots`], so
/// [`CommandExecution::with_snapshot`] can accept either.
pub trait IntoSnapshots<'a, C>: Sized
where
    C: Decider,
{
    /// The resulting configuration's snapshot store type.
    type Store;
    /// The resulting configuration's snapshot policy type.
    type Policy;
    /// The resulting configuration's snapshot task scheduler type.
    type SnapshotTaskScheduler;
    /// The resulting configuration's snapshot failure policy type.
    type FailurePolicy;

    /// Builds the [`Snapshots`] configuration.
    fn into_snapshots(
        self,
    ) -> Snapshots<'a, Self::Store, Self::Policy, Self::SnapshotTaskScheduler, Self::FailurePolicy>;
}

impl<'a, C, S, P, Spawn, F> IntoSnapshots<'a, C> for Snapshots<'a, S, P, Spawn, F>
where
    C: Decider,
{
    type Store = S;
    type Policy = P;
    type SnapshotTaskScheduler = Spawn;
    type FailurePolicy = F;

    fn into_snapshots(
        self,
    ) -> Snapshots<'a, Self::Store, Self::Policy, Self::SnapshotTaskScheduler, Self::FailurePolicy> {
        self
    }
}

impl<'a, C, S> IntoSnapshots<'a, C> for &'a S
where
    C: CommandSnapshotPolicy,
    C::State: SnapshotType,
{
    type Store = S;
    type Policy = C::SnapshotPolicy;
    type SnapshotTaskScheduler = WithoutSnapshotTaskScheduler;
    type FailurePolicy = FailOnSnapshotFailure;

    fn into_snapshots(
        self,
    ) -> Snapshots<'a, Self::Store, Self::Policy, Self::SnapshotTaskScheduler, Self::FailurePolicy> {
        C::snapshots(self)
    }
}

/// Runtime boundary that applies one [`Decider`] command to one stream: reads
/// the stream (and optionally a snapshot), replays state, decides the next
/// events, and appends them with the configured write precondition.
///
/// Build one with [`CommandExecution::new`] and configure it with the
/// builder methods before calling `execute`, which is available when `S` is
/// [`WithoutSnapshots`] or a [`Snapshots`] configuration whose store and
/// policy satisfy the trait bounds that method requires.
pub struct CommandExecution<'a, E, C, S, G, A = WithoutAdmission, Auth = WithoutAuthorization> {
    event_store: &'a E,
    command: &'a C,
    expected_revision: Option<StreamPosition>,
    snapshots: S,
    headers: Headers,
    command_id: Option<CommandId>,
    event_id_generator: G,
    replay_limit: Option<ReplayLimit>,
    replay_chunk_size: Option<ReplayChunkSize>,
    conflict_retry_limit: Option<ConflictRetryLimit>,
    admission: A,
    principal: Option<CommandPrincipal>,
    authorizer: Auth,
}

impl<'a, E, C> CommandExecution<'a, E, C, WithoutSnapshots, UuidV7Generator>
where
    C: Decider,
{
    /// Starts building an execution for `command` against `event_store`, with
    /// no snapshot store, no explicit write precondition, no headers, no
    /// admission limit, no authorization, and the default UUIDv7 event id
    /// generator.
    pub fn new(event_store: &'a E, command: &'a C) -> Self {
        Self {
            event_store,
            command,
            expected_revision: None,
            snapshots: WithoutSnapshots,
            headers: Headers::empty(),
            command_id: None,
            event_id_generator: UuidV7Generator,
            replay_limit: None,
            replay_chunk_size: None,
            conflict_retry_limit: None,
            admission: WithoutAdmission,
            principal: None,
            authorizer: WithoutAuthorization,
        }
    }
}

impl<'a, E, C, G, A, Auth> CommandExecution<'a, E, C, WithoutSnapshots, G, A, Auth>
where
    C: Decider,
{
    /// Configures this execution to read and write snapshots, accepting
    /// either a bare snapshot store (using the decider's
    /// [`CommandSnapshotPolicy`]) or an explicitly built [`Snapshots`].
    #[allow(clippy::type_complexity)]
    pub fn with_snapshot<I>(
        self,
        snapshots: I,
    ) -> CommandExecution<
        'a,
        E,
        C,
        Snapshots<
            'a,
            <I as IntoSnapshots<'a, C>>::Store,
            <I as IntoSnapshots<'a, C>>::Policy,
            <I as IntoSnapshots<'a, C>>::SnapshotTaskScheduler,
            <I as IntoSnapshots<'a, C>>::FailurePolicy,
        >,
        G,
        A,
        Auth,
    >
    where
        I: IntoSnapshots<'a, C>,
    {
        CommandExecution {
            event_store: self.event_store,
            command: self.command,
            expected_revision: self.expected_revision,
            snapshots: snapshots.into_snapshots(),
            headers: self.headers,
            command_id: self.command_id,
            event_id_generator: self.event_id_generator,
            replay_limit: self.replay_limit,
            replay_chunk_size: self.replay_chunk_size,
            conflict_retry_limit: self.conflict_retry_limit,
            admission: self.admission,
            principal: self.principal,
            authorizer: self.authorizer,
        }
    }
}

impl<'a, E, C, S, G, A, Auth> CommandExecution<'a, E, C, S, G, A, Auth> {
    /// Records the stream revision a client observed before issuing this command.
    ///
    /// Use this when the request carries a client's own read position, such as an `If-Match`
    /// header. It strengthens the command's declared [`WritePrecondition`] rather than replacing
    /// it: the append is guarded on that exact revision, so a stream that has advanced since the
    /// client read it rejects the write. A command declaring
    /// [`WritePrecondition::NoStream`] fails with
    /// [`CommandError::PreconditionConflict`] instead, because it can only create a stream that
    /// has no revision yet.
    pub fn with_expected_revision<R>(mut self, expected_revision: R) -> Self
    where
        R: Into<Option<StreamPosition>>,
    {
        self.expected_revision = expected_revision.into();
        self
    }

    /// Sets metadata headers attached to every event this execution appends.
    pub fn with_headers(mut self, headers: Headers) -> Self {
        self.headers = headers;
        self
    }

    /// Caps the number of stream events this execution may replay.
    ///
    /// Defaults to unlimited. When set, a stream read that returns more than
    /// `replay_limit` events fails the command with
    /// [`CommandError::ReplayLimitExceeded`] before folding any of them into
    /// state.
    pub fn with_replay_limit(mut self, replay_limit: ReplayLimit) -> Self {
        self.replay_limit = Some(replay_limit);
        self
    }

    /// Replays history in reads of at most `replay_chunk_size` events, folding
    /// and dropping each before fetching the next.
    ///
    /// Defaults to unset, which reads the whole history after the snapshot at
    /// once and holds all of it while folding, so peak memory follows the
    /// stream. Set this to make peak memory follow the chunk instead.
    ///
    /// This does not change what a command decides. The walk is pinned to the
    /// position the first read observed, so a stream that grows while the chunks
    /// are being read contributes nothing to this execution and conflicts on the
    /// append exactly as it would have unchunked.
    ///
    /// It composes with [`with_replay_limit`](Self::with_replay_limit), with one
    /// consequence worth naming: an unchunked replay rejects an over-limit
    /// stream before folding anything, while a chunked one has already folded
    /// the chunks before the limit was reached. The command still fails with
    /// [`CommandError::ReplayLimitExceeded`] and still appends nothing.
    pub fn with_replay_chunk_size(mut self, replay_chunk_size: ReplayChunkSize) -> Self {
        self.replay_chunk_size = Some(replay_chunk_size);
        self
    }

    /// The replay limit and chunk size this execution was configured with.
    fn replay_bounds(&self) -> ReplayBounds {
        ReplayBounds::new(self.replay_limit, self.replay_chunk_size)
    }

    /// Re-reads and re-decides in place when another writer beats this
    /// execution to the stream, up to `conflict_retry_limit` extra attempts.
    ///
    /// Defaults to unset, which is the behaviour every caller has today: the
    /// conflict comes back as [`CommandError::Append`] and re-running the
    /// command is the caller's decision. Set this where the caller would only
    /// re-issue the same command anyway, so two writers on one stream stop
    /// producing caller-visible errors that the runtime could have resolved.
    ///
    /// Three conditions must all hold before an attempt is retried, and the
    /// limit only controls the last of them.
    ///
    /// The store must say the failure was a conflict, through
    /// [`StreamAppend::classify_append_failure`]. A store that has not
    /// classified its errors reports every append failure as
    /// [`AppendFailure::Fatal`], so setting this changes nothing for it.
    ///
    /// The command must declare [`WritePrecondition::StreamUnchanged`], and
    /// this execution must carry no
    /// [`with_expected_revision`](Self::with_expected_revision). Those are the
    /// cases where the guard came from the state this execution read, so
    /// reading again produces a genuinely new decision. Every other
    /// precondition asserts something about the stream that a re-read cannot
    /// change: `NoStream` conflicts because the stream now exists,
    /// `StreamExists` because it does not, `Any` never conflicts at all, and a
    /// caller-supplied revision is the caller's own assertion to keep or
    /// abandon.
    ///
    /// A retried attempt repeats the whole round: the snapshot read, the
    /// stream read, the fold, and the decide. The decider may therefore reject
    /// the command on a later attempt, having seen state it had not seen
    /// before, and that rejection is the answer. Events keep their identity
    /// across attempts when
    /// [`with_command_id`](Self::with_command_id) is set, so a store that
    /// deduplicates on event id still recognizes a redelivery of the same
    /// command.
    pub fn with_conflict_retry(mut self, conflict_retry_limit: ConflictRetryLimit) -> Self {
        self.conflict_retry_limit = Some(conflict_retry_limit);
        self
    }

    /// Assigns event ids by deriving them from the command's own identity instead of generating
    /// fresh ones.
    ///
    /// Set this whenever the command arrives over an at-least-once transport. The events a
    /// redelivered command decides then carry the same ids as its first attempt, which is what lets
    /// a storage adapter keyed on event identity (`trogon-decider-nats` publishes it as the
    /// JetStream `Nats-Msg-Id`) recognize the retry and drop the duplicate append.
    ///
    /// Left unset, ids come from the UUIDv7 generator and differ on every attempt, which is only
    /// safe for a command constructed in-process where no redelivery exists.
    pub fn with_command_id<I>(mut self, command_id: I) -> Self
    where
        I: Into<Option<CommandId>>,
    {
        self.command_id = command_id.into();
        self
    }

    /// Replaces the generator used to assign ids to newly decided events that
    /// don't already carry one, and that no [`with_command_id`](Self::with_command_id) derivation
    /// covers.
    pub fn with_event_id_generator<NextG>(
        self,
        event_id_generator: NextG,
    ) -> CommandExecution<'a, E, C, S, NextG, A, Auth>
    where
        NextG: NowV7,
    {
        CommandExecution {
            event_store: self.event_store,
            command: self.command,
            expected_revision: self.expected_revision,
            snapshots: self.snapshots,
            headers: self.headers,
            command_id: self.command_id,
            event_id_generator,
            replay_limit: self.replay_limit,
            replay_chunk_size: self.replay_chunk_size,
            conflict_retry_limit: self.conflict_retry_limit,
            admission: self.admission,
            principal: self.principal,
            authorizer: self.authorizer,
        }
    }

    /// Gates this execution on a [`CommandAdmission`] limiter.
    ///
    /// Defaults to [`WithoutAdmission`]: execution is unbounded, exactly as it
    /// was before admission control existed. Once set, the limiter is
    /// consulted before any stream read, decide, or append, and a command it
    /// cannot admit fails immediately with [`CommandError::Overloaded`]
    /// instead of queueing.
    ///
    /// Pass a shared [`ConcurrencyAdmission`] (or a reference to one) so every
    /// execution in the process counts against one host budget. See
    /// [ADR#0028](https://github.com/TrogonStack/trogonai/blob/main/docs/adr/0028-decider-admission-control-and-backpressure.md)
    /// for why no bound is chosen here.
    pub fn with_admission<NextA>(self, admission: NextA) -> CommandExecution<'a, E, C, S, G, NextA, Auth>
    where
        NextA: CommandAdmission,
    {
        CommandExecution {
            event_store: self.event_store,
            command: self.command,
            expected_revision: self.expected_revision,
            snapshots: self.snapshots,
            headers: self.headers,
            command_id: self.command_id,
            event_id_generator: self.event_id_generator,
            replay_limit: self.replay_limit,
            replay_chunk_size: self.replay_chunk_size,
            conflict_retry_limit: self.conflict_retry_limit,
            admission,
            principal: self.principal,
            authorizer: self.authorizer,
        }
    }

    /// Names the [`CommandPrincipal`] this command is executed on behalf of.
    ///
    /// Only read by a configured [`CommandAuthorizer`]. It is not persisted:
    /// [`Headers`] remains the envelope metadata contract, so an application
    /// that wants an audit trail of who acted derives its own header from the
    /// principal and sets it with [`with_headers`](Self::with_headers).
    ///
    /// Setting this without an authorizer configured changes nothing, and
    /// configuring an authorizer without setting this fails every execution
    /// with [`UnauthorizedError::MissingPrincipal`].
    pub fn with_principal<P>(mut self, principal: P) -> Self
    where
        P: Into<Option<CommandPrincipal>>,
    {
        self.principal = principal.into();
        self
    }

    /// Gates this execution on a [`CommandAuthorizer`].
    ///
    /// Defaults to [`WithoutAuthorization`]: any caller able to build an
    /// execution may run any command, exactly as it was before authorization
    /// existed. Once set, the authorizer is consulted after the admission
    /// permit and before the first snapshot read, stream read, decide, or
    /// append, and a command it refuses fails with
    /// [`CommandError::Unauthorized`].
    ///
    /// The call happens once per execution, outside the conflict-retry loop, so
    /// a command that re-reads and decides again is still authorized once.
    ///
    /// Pass a shared authorizer (a reference or an [`Arc`](std::sync::Arc)) to
    /// evaluate every execution in the process against one policy. See
    /// [ADR#0026](https://github.com/TrogonStack/trogonai/blob/main/docs/adr/0026-command-authorization-principal.md)
    /// for why no policy is chosen here.
    pub fn with_authorizer<NextAuth>(self, authorizer: NextAuth) -> CommandExecution<'a, E, C, S, G, A, NextAuth>
    where
        NextAuth: CommandAuthorizer<C>,
    {
        CommandExecution {
            event_store: self.event_store,
            command: self.command,
            expected_revision: self.expected_revision,
            snapshots: self.snapshots,
            headers: self.headers,
            command_id: self.command_id,
            event_id_generator: self.event_id_generator,
            replay_limit: self.replay_limit,
            replay_chunk_size: self.replay_chunk_size,
            conflict_retry_limit: self.conflict_retry_limit,
            admission: self.admission,
            principal: self.principal,
            authorizer,
        }
    }
}

impl<E, C, S, G, A, Auth> CommandExecution<'_, E, C, S, G, A, Auth> {
    async fn append_decision(
        &self,
        current_position: Option<StreamPosition>,
        stream_id: &C::StreamId,
        state: C::State,
    ) -> Result<
        (AppendStreamResponse, Events<C::Event>, C::State),
        AppendDecisionError<
            C::DecideError,
            C::EvolveError,
            CommandAppendStreamError<E, C>,
            CommandEventTypeError<C>,
            CommandEventPayloadEncodeError<C>,
        >,
    >
    where
        C: Decider,
        C::Event: Clone + EventType + EventIdentity + EventEncode,
        E: StreamAppend<C::StreamId>,
        G: NowV7,
        CommandEventTypeError<C>: std::error::Error + Send + Sync + 'static,
        CommandEventPayloadEncodeError<C>: std::error::Error + Send + Sync + 'static,
    {
        let stream_write_precondition =
            resolve_write_precondition(C::WRITE_PRECONDITION, self.expected_revision, current_position)
                .map_err(AppendDecisionError::PreconditionConflict)?;
        tracing::Span::current().record(
            attribute::WRITE_PRECONDITION,
            write_precondition_attribute(stream_write_precondition).as_str(),
        );

        let (state, events) = evaluate_decision(state, self.command).map_err(|failure| match failure {
            DecisionError::Decide(error) => AppendDecisionError::Decide(error),
            DecisionError::Evolve(error) => AppendDecisionError::Evolve(error),
        })?;
        let mut encoded_events = Vec::with_capacity(events.len());
        for (index, event) in events.iter().enumerate() {
            let id = event.event_id().unwrap_or_else(|| match self.command_id {
                Some(command_id) => command_id.event_id(index),
                None => EventId::new(self.event_id_generator.now_v7()),
            });
            encoded_events.push(Event {
                id,
                r#type: event.event_type().map_err(AppendDecisionError::EventType)?.to_string(),
                content: event.encode().map_err(AppendDecisionError::EventEncode)?,
                headers: self.headers.clone(),
            });
        }

        let append_outcome = self
            .event_store
            .append_stream(AppendStreamRequest {
                stream_id,
                stream_write_precondition,
                events: encoded_events,
            })
            .await
            .map_err(AppendDecisionError::Append)?;

        Ok((append_outcome, events, state))
    }
}

impl<'a, E, S, C, P, Spawn, F, G, A, Auth> CommandExecution<'a, E, C, Snapshots<'a, S, P, Spawn, F>, G, A, Auth> {
    /// Replaces the scheduler used to run snapshot write tasks.
    pub fn with_task_runtime<NextSpawn>(
        self,
        schedule_snapshot_task: NextSpawn,
    ) -> CommandExecution<'a, E, C, Snapshots<'a, S, P, NextSpawn, F>, G, A, Auth> {
        CommandExecution {
            event_store: self.event_store,
            command: self.command,
            expected_revision: self.expected_revision,
            snapshots: self.snapshots.schedule_snapshot_tasks_with(schedule_snapshot_task),
            headers: self.headers,
            command_id: self.command_id,
            event_id_generator: self.event_id_generator,
            replay_limit: self.replay_limit,
            replay_chunk_size: self.replay_chunk_size,
            conflict_retry_limit: self.conflict_retry_limit,
            admission: self.admission,
            principal: self.principal,
            authorizer: self.authorizer,
        }
    }

    /// Sets how this execution reacts to a snapshot it cannot trust.
    ///
    /// Defaults to [`FailOnSnapshotFailure`], which fails the command exactly
    /// as before this policy existed. Use [`DiscardAndReplaySnapshotFailure`]
    /// or a custom [`SnapshotFailurePolicy`] to recover instead.
    pub fn with_snapshot_failure_policy<NextF>(
        self,
        failure_policy: NextF,
    ) -> CommandExecution<'a, E, C, Snapshots<'a, S, P, Spawn, NextF>, G, A, Auth> {
        CommandExecution {
            event_store: self.event_store,
            command: self.command,
            expected_revision: self.expected_revision,
            snapshots: self.snapshots.with_snapshot_failure_policy(failure_policy),
            headers: self.headers,
            command_id: self.command_id,
            event_id_generator: self.event_id_generator,
            replay_limit: self.replay_limit,
            replay_chunk_size: self.replay_chunk_size,
            conflict_retry_limit: self.conflict_retry_limit,
            admission: self.admission,
            principal: self.principal,
            authorizer: self.authorizer,
        }
    }
}

impl<E, C, S, G, A, Auth> CommandExecution<'_, E, C, S, G, A, Auth>
where
    C: Decider + Sync,
    C::Event: Clone + EventType + EventIdentity + EventEncode + EventDecode,
    C::StreamId: std::fmt::Display + Sync,
    E: StreamRead<C::StreamId> + StreamAppend<C::StreamId>,
    S: ExecutionSnapshots<C>,
    G: NowV7,
    A: CommandAdmission,
    Auth: CommandAuthorizer<C>,
    CommandEventTypeError<C>: std::error::Error + Send + Sync + 'static,
    CommandEventPayloadEncodeError<C>: std::error::Error + Send + Sync + 'static,
    CommandEventDecodeError<C>: std::error::Error + Send + Sync + 'static,
{
    /// Applies the command to its stream: loads the state to resume from,
    /// replays the events recorded after it, decides, and appends the result
    /// under the resolved write precondition.
    ///
    /// What the surrounding configuration adds is the snapshot half. Without
    /// [`with_snapshot`](Self::with_snapshot) the stream is replayed from the
    /// beginning and nothing is persisted. With it, a snapshot is loaded so
    /// only the events after it are replayed, and a new one is scheduled when
    /// the configured [`SnapshotPolicy`] decides to take it. A snapshot the
    /// [`SnapshotFailurePolicy`] cannot trust is discarded, replayed past, and
    /// replaced.
    ///
    /// A [`with_admission`](Self::with_admission) limiter, if one is
    /// configured, gates all of that: the permit is taken before the first
    /// read and released when this future resolves. A
    /// [`with_authorizer`](Self::with_authorizer) authorizer, if one is
    /// configured, is consulted immediately after that permit, so a denied
    /// command reads nothing and appends nothing.
    pub async fn execute(
        self,
    ) -> CommandResult<C, S::ReadError, CommandReadStreamError<E, C>, CommandAppendStreamError<E, C>> {
        let span = tracing::info_span!(
            span::DECIDER_EXECUTE_COMMAND,
            stream_id = tracing::field::Empty,
            write_precondition = tracing::field::Empty,
            decision_outcome = tracing::field::Empty,
        );

        let result = self.execute_inner().instrument(span.clone()).await;
        let decision_outcome = match &result {
            Ok(_) => attribute::DecisionOutcome::Decided,
            Err(error) => decision_outcome_for_error(error),
        };
        span.record(attribute::DECISION_OUTCOME, decision_outcome.as_str());
        result
    }

    async fn execute_inner(
        self,
    ) -> CommandResult<C, S::ReadError, CommandReadStreamError<E, C>, CommandAppendStreamError<E, C>> {
        // Held until this execution ends, so the slot covers the reads, the
        // decide, and the append rather than only the moment of admission.
        // Held across retries too: a command that re-reads is still one
        // command occupying one slot.
        let _permit = self.admission.admit().map_err(CommandError::Overloaded)?;

        // Before the first read and outside the retry loop below: a denied
        // command costs one call, and a command that re-reads is still one
        // command answering to one authorization decision.
        self.authorizer
            .authorize_execution(self.principal.as_ref(), self.command)
            .map_err(CommandError::Unauthorized)?;

        let stream_id = self.command.stream_id();
        tracing::Span::current().record(attribute::STREAM_ID, tracing::field::display(stream_id));

        let mut retries_left = self.resolved_conflict_retry_budget();
        loop {
            let result = self.attempt(stream_id).await;

            let CommandError::Append(append_error) = (match result {
                Err(ref error) => error,
                Ok(_) => return result,
            }) else {
                return result;
            };
            if retries_left == 0
                || self.event_store.classify_append_failure(append_error) != AppendFailure::WriteConflict
            {
                return result;
            }

            retries_left -= 1;
            metrics().conflict_retries.add(1, &[]);
            tracing::debug!(
                stream_id = %stream_id,
                retries_left,
                "another writer advanced the stream; re-reading and deciding again"
            );
        }
    }

    /// How many retries this execution may spend, once the configured limit is
    /// checked against the preconditions that make a retry meaningful.
    ///
    /// Resolved once rather than per attempt, because none of its inputs can
    /// change while the command runs.
    fn resolved_conflict_retry_budget(&self) -> u32 {
        let retryable = C::WRITE_PRECONDITION == WritePrecondition::StreamUnchanged && self.expected_revision.is_none();
        match self.conflict_retry_limit {
            Some(limit) if retryable => limit.as_u32(),
            _ => 0,
        }
    }

    async fn attempt(
        &self,
        stream_id: &C::StreamId,
    ) -> CommandResult<C, S::ReadError, CommandReadStreamError<E, C>, CommandAppendStreamError<E, C>> {
        // A command that may only create its stream has no history to load: any history at all
        // would already violate the precondition the append is about to be guarded on. Skipping
        // both reads keeps creation a single round trip.
        let ReplayContext {
            state,
            snapshot_position,
            stream_events,
            current_position,
            discarded_snapshot,
        } = if C::WRITE_PRECONDITION == WritePrecondition::NoStream {
            ReplayContext {
                state: C::initial_state(),
                snapshot_position: None,
                stream_events: Vec::new(),
                current_position: None,
                discarded_snapshot: false,
            }
        } else {
            self.snapshots
                .load_replay_context(self.event_store, self.command, stream_id, self.replay_bounds())
                .await?
        };

        let mut cursor = ReplayCursor::new(self.replay_bounds(), current_position);
        let mut state = state;
        let mut chunk = stream_events;
        loop {
            cursor.truncate_to_tail(&mut chunk);
            let replayed_event_count = cursor.advance(&chunk);
            ensure_replay_within_limit(self.replay_limit, replayed_event_count)
                .map_err(CommandError::ReplayLimitExceeded)?;
            state = evolve_state_from_stream_events::<C>(state, &chunk)?;

            let Some(next_read) = cursor.next_read() else {
                break;
            };
            let from = next_read.map_err(CommandError::ReadAfterOverflow)?;
            chunk = read_stream_for_execution(
                self.event_store,
                ReadStreamRequest { stream_id, from },
                cursor.read_bound(),
            )
            .await
            .map_err(CommandError::ReadStream)?
            .events;
            if chunk.is_empty() {
                break;
            }
        }

        let replayed_event_count = cursor.replayed_event_count();
        metrics().replay_events.add(replayed_event_count, &[]);

        let (append_outcome, events, state) = self.append_decision(current_position, stream_id, state).await?;

        self.snapshots.store(
            stream_id,
            discarded_snapshot,
            DecideSnapshot {
                command: self.command,
                stream_position: append_outcome.stream_position,
                snapshot_position,
                state: &state,
                events: &events,
                replayed_event_count,
            },
        );

        Ok(ExecutionResult {
            stream_position: append_outcome.stream_position,
            events,
            state,
        })
    }
}

/// Rejects a command whose stream read returned more events than its
/// [`ReplayLimit`] allows to replay, before any of them are folded.
pub fn ensure_replay_within_limit(
    replay_limit: Option<ReplayLimit>,
    replayed_event_count: u64,
) -> Result<(), ReplayLimitExceeded> {
    match replay_limit {
        Some(limit) if replayed_event_count > limit.as_u64() => Err(ReplayLimitExceeded {
            limit,
            replayed_event_count,
        }),
        _ => Ok(()),
    }
}

/// Reads a stream for a command execution, bounding the read to one more
/// than the configured [`ReplayLimit`] when one is set.
///
/// Capping the fetched count at `limit + 1` is enough to detect that the
/// limit was exceeded without reading the rest of the stream: a response
/// with `limit + 1` events proves the true count is more than the limit,
/// which is exactly what [`ensure_replay_within_limit`] needs to reject the
/// command. With no configured limit, this reads the stream unbounded, same
/// as before.
pub async fn read_stream_for_execution<StreamId, E>(
    event_store: &E,
    request: ReadStreamRequest<'_, StreamId>,
    read_bound: Option<u64>,
) -> Result<ReadStreamResponse, E::Error>
where
    StreamId: ?Sized,
    E: StreamRead<StreamId>,
{
    match read_bound {
        Some(max_events) => event_store.read_stream_bounded(request, max_events).await,
        None => event_store.read_stream(request).await,
    }
}

/// Rejects a snapshot claiming a position the stream has not reached, which
/// means the snapshot cannot be trusted to summarize the events before it.
pub fn ensure_snapshot_not_ahead(
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

fn maybe_take_snapshot<S, C, P, Spawn, F>(
    snapshots: &Snapshots<'_, S, P, Spawn, F>,
    stream_id: &C::StreamId,
    context: DecideSnapshot<'_, C>,
) where
    C: Decider,
    C::State: Clone + SnapshotType + Send + 'static,
    C::StreamId: std::fmt::Display + ToOwned,
    <C::StreamId as ToOwned>::Owned: Borrow<C::StreamId> + Send + 'static,
    S: Clone + SnapshotWrite<C::State, C::StreamId> + 'static,
    S::Error: std::fmt::Display + Send + 'static,
    P: SnapshotPolicy<C>,
    Spawn: SnapshotTaskScheduler + Send + Sync,
{
    let stream_position = context.stream_position;
    let state = context.state;
    let snapshot_decision = snapshots.policy.decide_snapshot(context);

    if snapshot_decision == SnapshotDecision::Take {
        schedule_snapshot_write(
            &snapshots.schedule_snapshot_task,
            snapshots.snapshot_store,
            stream_id,
            Snapshot::new(stream_position, state.clone()),
        );
    }
}

fn schedule_snapshot_write<S, State, StreamId, Spawn>(
    schedule_snapshot_task: &Spawn,
    snapshot_store: &S,
    stream_id: &StreamId,
    snapshot: Snapshot<State>,
) where
    S: SnapshotWrite<State, StreamId> + Clone + Send + Sync + 'static,
    S::Error: std::fmt::Display + Send + 'static,
    State: SnapshotType + Send + 'static,
    StreamId: std::fmt::Display + ToOwned + ?Sized,
    StreamId::Owned: Borrow<StreamId> + Send + 'static,
    Spawn: SnapshotTaskScheduler + Send + Sync,
{
    let snapshot_store = snapshot_store.clone();
    let stream_id_for_log = stream_id.to_string();
    let stream_id = stream_id.to_owned();

    schedule_snapshot_task.schedule(async move {
        let span = tracing::info_span!(
            span::DECIDER_WRITE_SNAPSHOT,
            stream_id = %stream_id_for_log,
            snapshot_write_success = tracing::field::Empty,
        );
        let result = snapshot_store
            .write_snapshot(WriteSnapshotRequest {
                snapshot_id: stream_id.borrow(),
                snapshot,
            })
            .instrument(span.clone())
            .await;
        let success = result.is_ok();
        span.record(attribute::SNAPSHOT_WRITE_SUCCESS, success);
        metrics()
            .snapshot_writes
            .add(1, &[KeyValue::new(attribute::SNAPSHOT_WRITE_SUCCESS, success)]);
        if let Err(source) = result {
            tracing::warn!(stream_id = %stream_id_for_log, error = %source, "failed to write snapshot");
        }
    });
}

#[allow(
    clippy::disallowed_methods,
    reason = "decider runtime replay path; the disallowed_methods rule targets test code calling decide/evolve directly"
)]
fn evolve_state_from_stream_events<C>(
    mut state: C::State,
    stream_events: &[StreamEvent],
) -> Result<C::State, ReplayStreamError<C::EvolveError, CommandEventDecodeError<C>>>
where
    C: Decider,
    C::Event: EventDecode,
    CommandEventDecodeError<C>: std::error::Error + Send + Sync + 'static,
{
    for stream_event in stream_events {
        match stream_event
            .decode::<C::Event>()
            .map_err(ReplayStreamError::DecodeEvent)?
        {
            EventDecodeOutcome::Decoded(event) => {
                state = C::evolve(state, &event).map_err(ReplayStreamError::Evolve)?;
            }
            // Shared or migrated streams may contain envelopes outside this
            // decider's event set; those still count toward stream position,
            // but they must not affect this decider's state.
            EventDecodeOutcome::Skipped => {}
        }
    }

    Ok(state)
}

#[cfg(test)]
mod tests;
