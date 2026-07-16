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

use crate::snapshot::{ReadSnapshotRequest, Snapshot, SnapshotRead, SnapshotType, SnapshotWrite, WriteSnapshotRequest};
use crate::stream::{
    AppendStreamRequest, AppendStreamResponse, ReadAfterOverflow, ReadFrom, ReadStreamRequest, StreamAppend,
    StreamPosition, StreamRead, StreamWritePrecondition,
};
use crate::{
    Decider, Event, EventDecode, EventDecodeOutcome, EventEncode, EventId, EventIdentity, EventType, Events, Headers,
    StreamEvent, WritePrecondition,
};
use trogon_decider::{DecisionFailure, evaluate_decision};
use trogon_std::{NowV7, UuidV7Generator};

use std::{
    borrow::Borrow,
    future::Future,
    num::NonZeroU64,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
type CommandEventTypeError<C> = <<C as Decider>::Event as EventType>::Error;
type CommandEventPayloadEncodeError<C> = <<C as Decider>::Event as EventEncode>::Error;
type CommandEventDecodeError<C> = <<C as Decider>::Event as EventDecode>::Error;
type CommandReadStreamError<E, C> = <E as StreamRead<<C as Decider>::StreamId>>::Error;
type CommandAppendStreamError<E, C> = <E as StreamAppend<<C as Decider>::StreamId>>::Error;
type CommandReadSnapshotError<S, C> = <S as SnapshotRead<<C as Decider>::State, <C as Decider>::StreamId>>::Error;
type CommandWriteSnapshotError<S, C> = <S as SnapshotWrite<<C as Decider>::State, <C as Decider>::StreamId>>::Error;
type CommandWithoutSnapshotsResult<E, C> =
    CommandResult<C, std::convert::Infallible, CommandReadStreamError<E, C>, CommandAppendStreamError<E, C>>;
type CommandWithSnapshotsResult<E, S, C> =
    CommandResult<C, CommandReadSnapshotError<S, C>, CommandReadStreamError<E, C>, CommandAppendStreamError<E, C>>;

/// Schedules best-effort snapshot writes without erasing the task future type.
///
/// Snapshot execution owns the async block that writes the snapshot. A generic
/// scheduler keeps that future concrete instead of forcing every runtime adapter
/// through a boxed `dyn Future`.
pub trait SnapshotTaskScheduler {
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

        let tasks = Arc::clone(&self.tasks);
        tasks.in_flight.fetch_add(1, Ordering::SeqCst);
        drop(handle.spawn(async move {
            task.await;
            if tasks.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
                tasks.idle.notify_waiters();
            }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotDecision {
    Skip,
    Take,
}

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
    pub state: &'a C::State,
    pub events: &'a Events<C::Event>,
    /// Number of persisted stream events read after the snapshot position.
    pub replayed_event_count: u64,
}

pub trait SnapshotPolicy<C: Decider> {
    fn decide_snapshot(&self, context: DecideSnapshot<'_, C>) -> SnapshotDecision;
}

pub trait CommandSnapshotPolicy: Decider
where
    Self::State: SnapshotType,
{
    type SnapshotPolicy: SnapshotPolicy<Self>;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy;

    fn snapshots<'a, S>(snapshot_store: &'a S) -> Snapshots<'a, S, Self::SnapshotPolicy> {
        Snapshots::new(snapshot_store, Self::SNAPSHOT_POLICY)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoSnapshot;

impl<C: Decider> SnapshotPolicy<C> for NoSnapshot {
    fn decide_snapshot(&self, _context: DecideSnapshot<'_, C>) -> SnapshotDecision {
        SnapshotDecision::Skip
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrequencySnapshot {
    frequency: NonZeroU64,
}

impl FrequencySnapshot {
    pub const fn new(frequency: NonZeroU64) -> Self {
        Self { frequency }
    }

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
    ReadAfterOverflow(#[source] ReadAfterOverflow),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotAheadOfStream {
    pub snapshot_position: StreamPosition,
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
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WithoutSnapshots;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WithoutSnapshotTaskScheduler;

pub struct Snapshots<'a, S, P, Spawn = WithoutSnapshotTaskScheduler, F = FailOnSnapshotFailure> {
    snapshot_store: &'a S,
    policy: P,
    schedule_snapshot_task: Spawn,
    failure_policy: F,
}

impl<'a, S, P> Snapshots<'a, S, P, WithoutSnapshotTaskScheduler, FailOnSnapshotFailure> {
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

pub trait IntoSnapshots<'a, C>: Sized
where
    C: Decider,
{
    type Store;
    type Policy;
    type SnapshotTaskScheduler;
    type FailurePolicy;

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

pub struct CommandExecution<'a, E, C, S, G> {
    event_store: &'a E,
    command: &'a C,
    write_precondition: Option<StreamWritePrecondition>,
    snapshots: S,
    headers: Headers,
    event_id_generator: G,
}

impl<'a, E, C> CommandExecution<'a, E, C, WithoutSnapshots, UuidV7Generator>
where
    C: Decider,
{
    pub fn new(event_store: &'a E, command: &'a C) -> Self {
        Self {
            event_store,
            command,
            write_precondition: None,
            snapshots: WithoutSnapshots,
            headers: Headers::empty(),
            event_id_generator: UuidV7Generator,
        }
    }
}

impl<'a, E, C, G> CommandExecution<'a, E, C, WithoutSnapshots, G>
where
    C: Decider,
{
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
    >
    where
        I: IntoSnapshots<'a, C>,
    {
        CommandExecution {
            event_store: self.event_store,
            command: self.command,
            write_precondition: self.write_precondition,
            snapshots: snapshots.into_snapshots(),
            headers: self.headers,
            event_id_generator: self.event_id_generator,
        }
    }
}

impl<'a, E, C, S, G> CommandExecution<'a, E, C, S, G> {
    pub fn with_write_precondition<W>(mut self, write_precondition: W) -> Self
    where
        W: Into<Option<StreamWritePrecondition>>,
    {
        self.write_precondition = write_precondition.into();
        self
    }

    pub fn with_headers(mut self, headers: Headers) -> Self {
        self.headers = headers;
        self
    }

    pub fn with_event_id_generator<NextG>(self, event_id_generator: NextG) -> CommandExecution<'a, E, C, S, NextG>
    where
        NextG: NowV7,
    {
        CommandExecution {
            event_store: self.event_store,
            command: self.command,
            write_precondition: self.write_precondition,
            snapshots: self.snapshots,
            headers: self.headers,
            event_id_generator,
        }
    }
}

impl<'a, E, C, S, G> CommandExecution<'a, E, C, S, G> {
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
        C::StreamId: AsRef<str>,
        E: StreamAppend<C::StreamId>,
        G: NowV7,
        CommandEventTypeError<C>: std::error::Error + Send + Sync + 'static,
        CommandEventPayloadEncodeError<C>: std::error::Error + Send + Sync + 'static,
    {
        let (state, events) = evaluate_decision(state, self.command).map_err(|failure| match failure {
            DecisionFailure::Decide(error) => AppendDecisionError::Decide(error),
            DecisionFailure::Evolve(error) => AppendDecisionError::Evolve(error),
        })?;
        let mut encoded_events = Vec::with_capacity(events.len());
        for event in events.iter() {
            let id = event
                .event_id()
                .unwrap_or_else(|| EventId::new(self.event_id_generator.now_v7()));
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
                stream_write_precondition: C::WRITE_PRECONDITION
                    .map(StreamWritePrecondition::from)
                    .or(self.write_precondition)
                    .unwrap_or_else(|| current_position.into()),
                events: encoded_events,
            })
            .await
            .map_err(AppendDecisionError::Append)?;

        Ok((append_outcome, events, state))
    }
}

impl<'a, E, S, C, P, Spawn, F, G> CommandExecution<'a, E, C, Snapshots<'a, S, P, Spawn, F>, G> {
    pub fn with_task_runtime<NextSpawn>(
        self,
        schedule_snapshot_task: NextSpawn,
    ) -> CommandExecution<'a, E, C, Snapshots<'a, S, P, NextSpawn, F>, G> {
        CommandExecution {
            event_store: self.event_store,
            command: self.command,
            write_precondition: self.write_precondition,
            snapshots: self.snapshots.schedule_snapshot_tasks_with(schedule_snapshot_task),
            headers: self.headers,
            event_id_generator: self.event_id_generator,
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
    ) -> CommandExecution<'a, E, C, Snapshots<'a, S, P, Spawn, NextF>, G> {
        CommandExecution {
            event_store: self.event_store,
            command: self.command,
            write_precondition: self.write_precondition,
            snapshots: self.snapshots.with_snapshot_failure_policy(failure_policy),
            headers: self.headers,
            event_id_generator: self.event_id_generator,
        }
    }
}

impl<E, C, G> CommandExecution<'_, E, C, WithoutSnapshots, G>
where
    C: Decider,
    C::Event: Clone + EventType + EventIdentity + EventEncode + EventDecode,
    C::StreamId: AsRef<str>,
    E: StreamRead<C::StreamId> + StreamAppend<C::StreamId>,
    G: NowV7,
    CommandEventTypeError<C>: std::error::Error + Send + Sync + 'static,
    CommandEventPayloadEncodeError<C>: std::error::Error + Send + Sync + 'static,
    CommandEventDecodeError<C>: std::error::Error + Send + Sync + 'static,
{
    pub async fn execute(self) -> CommandWithoutSnapshotsResult<E, C> {
        let stream_id = self.command.stream_id();
        if has_no_stream_write_precondition::<C>() {
            let (append_outcome, events, state) = self.append_decision(None, stream_id, C::initial_state()).await?;

            return Ok(ExecutionResult {
                stream_position: append_outcome.stream_position,
                events,
                state,
            });
        }

        let stream_read = self
            .event_store
            .read_stream(ReadStreamRequest {
                stream_id,
                from: ReadFrom::Beginning,
            })
            .await
            .map_err(CommandError::ReadStream)?;
        let current_position = stream_read.current_position;
        let state = evolve_state_from_stream_events::<C>(C::initial_state(), &stream_read.events)?;
        let (append_outcome, events, state) = self.append_decision(current_position, stream_id, state).await?;

        Ok(ExecutionResult {
            stream_position: append_outcome.stream_position,
            events,
            state,
        })
    }
}

impl<E, S, C, P, Spawn, F, G> CommandExecution<'_, E, C, Snapshots<'_, S, P, Spawn, F>, G>
where
    C: Decider,
    C::State: Clone + Send + 'static,
    C::Event: Clone + EventType + EventIdentity + EventEncode + EventDecode,
    C::StreamId: AsRef<str> + ToOwned,
    <C::StreamId as ToOwned>::Owned: Borrow<C::StreamId> + Send + 'static,
    E: StreamRead<C::StreamId> + StreamAppend<C::StreamId>,
    S: Clone + SnapshotRead<C::State, C::StreamId> + SnapshotWrite<C::State, C::StreamId> + 'static,
    P: SnapshotPolicy<C>,
    Spawn: SnapshotTaskScheduler + Send + Sync,
    F: SnapshotFailurePolicy<C, CommandReadSnapshotError<S, C>>,
    G: NowV7,
    CommandWriteSnapshotError<S, C>: std::fmt::Display + Send + 'static,
    CommandEventTypeError<C>: std::error::Error + Send + Sync + 'static,
    CommandEventPayloadEncodeError<C>: std::error::Error + Send + Sync + 'static,
    CommandEventDecodeError<C>: std::error::Error + Send + Sync + 'static,
    C::State: SnapshotType,
{
    pub async fn execute(self) -> CommandWithSnapshotsResult<E, S, C> {
        let stream_id = self.command.stream_id();
        if has_no_stream_write_precondition::<C>() {
            let (append_outcome, events, state) = self.append_decision(None, stream_id, C::initial_state()).await?;

            maybe_take_snapshot(
                &self.snapshots,
                stream_id,
                DecideSnapshot {
                    command: self.command,
                    stream_position: append_outcome.stream_position,
                    snapshot_position: None,
                    state: &state,
                    events: &events,
                    replayed_event_count: 0,
                },
            );

            return Ok(ExecutionResult {
                stream_position: append_outcome.stream_position,
                events,
                state,
            });
        }

        let mut discarded_bad_snapshot = false;
        let (mut snapshot_position, mut state) = match self
            .snapshots
            .snapshot_store
            .read_snapshot(ReadSnapshotRequest { snapshot_id: stream_id })
            .await
        {
            Ok(response) => {
                let snapshot = response.snapshot;
                let snapshot_position = snapshot.as_ref().map(|snapshot| snapshot.position);
                let state = snapshot
                    .map(|snapshot| snapshot.payload)
                    .unwrap_or_else(C::initial_state);
                (snapshot_position, state)
            }
            Err(error) => {
                let decision = self
                    .snapshots
                    .failure_policy
                    .decide_snapshot_failure(SnapshotFailureContext {
                        command: self.command,
                        failure: SnapshotFailure::ReadFailed(&error),
                    });
                match decision {
                    SnapshotFailureDecision::Fail => return Err(CommandError::ReadSnapshot(error)),
                    SnapshotFailureDecision::DiscardAndReplay => {
                        discarded_bad_snapshot = true;
                        (None, C::initial_state())
                    }
                }
            }
        };

        let from = match snapshot_position {
            Some(position) => ReadFrom::after(position).map_err(CommandError::ReadAfterOverflow)?,
            None => ReadFrom::Beginning,
        };
        let stream_read = self
            .event_store
            .read_stream(ReadStreamRequest { stream_id, from })
            .await
            .map_err(CommandError::ReadStream)?;
        let mut current_position = stream_read.current_position;
        let mut stream_events = stream_read.events;

        if let Some(position) = snapshot_position
            && let Err(ahead_of_stream) = ensure_snapshot_not_ahead(position, current_position)
        {
            let decision = self
                .snapshots
                .failure_policy
                .decide_snapshot_failure(SnapshotFailureContext {
                    command: self.command,
                    failure: SnapshotFailure::AheadOfStream(ahead_of_stream),
                });
            match decision {
                SnapshotFailureDecision::Fail => return Err(CommandError::SnapshotAheadOfStream(ahead_of_stream)),
                SnapshotFailureDecision::DiscardAndReplay => {
                    discarded_bad_snapshot = true;
                    snapshot_position = None;
                    state = C::initial_state();

                    let replay = self
                        .event_store
                        .read_stream(ReadStreamRequest {
                            stream_id,
                            from: ReadFrom::Beginning,
                        })
                        .await
                        .map_err(CommandError::ReadStream)?;
                    current_position = replay.current_position;
                    stream_events = replay.events;
                }
            }
        }

        let state = evolve_state_from_stream_events::<C>(state, &stream_events)?;
        let (append_outcome, events, state) = self.append_decision(current_position, stream_id, state).await?;
        let replayed_event_count = stream_events.len() as u64;

        if discarded_bad_snapshot {
            // The discarded snapshot is still sitting in the store at this
            // stream id. The normal policy might choose to skip a snapshot
            // this time, which would leave that stale or undecodable payload
            // in place for the next execution to trip over again. Writing
            // unconditionally here overwrites it with a snapshot the recovered
            // execution can trust.
            schedule_snapshot_write(
                &self.snapshots.schedule_snapshot_task,
                self.snapshots.snapshot_store,
                stream_id,
                Snapshot::new(append_outcome.stream_position, state.clone()),
            );
        } else {
            maybe_take_snapshot(
                &self.snapshots,
                stream_id,
                DecideSnapshot {
                    command: self.command,
                    stream_position: append_outcome.stream_position,
                    snapshot_position,
                    state: &state,
                    events: &events,
                    replayed_event_count,
                },
            );
        }

        Ok(ExecutionResult {
            stream_position: append_outcome.stream_position,
            events,
            state,
        })
    }
}

impl From<WritePrecondition> for StreamWritePrecondition {
    fn from(value: WritePrecondition) -> Self {
        match value {
            WritePrecondition::Any => Self::Any,
            WritePrecondition::StreamExists => Self::StreamExists,
            WritePrecondition::NoStream => Self::NoStream,
        }
    }
}

fn has_no_stream_write_precondition<C: Decider>() -> bool {
    C::WRITE_PRECONDITION == Some(WritePrecondition::NoStream)
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

fn maybe_take_snapshot<S, C, P, Spawn, F>(
    snapshots: &Snapshots<'_, S, P, Spawn, F>,
    stream_id: &C::StreamId,
    context: DecideSnapshot<'_, C>,
) where
    C: Decider,
    C::State: Clone + SnapshotType + Send + 'static,
    C::StreamId: AsRef<str> + ToOwned,
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
    StreamId: AsRef<str> + ToOwned + ?Sized,
    StreamId::Owned: Borrow<StreamId> + Send + 'static,
    Spawn: SnapshotTaskScheduler + Send + Sync,
{
    let snapshot_store = snapshot_store.clone();
    let stream_id_for_log = stream_id.as_ref().to_string();
    let stream_id = stream_id.to_owned();

    schedule_snapshot_task.schedule(async move {
        if let Err(source) = snapshot_store
            .write_snapshot(WriteSnapshotRequest {
                snapshot_id: stream_id.borrow(),
                snapshot,
            })
            .await
        {
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
