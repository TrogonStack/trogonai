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

use std::{borrow::Borrow, future::Future, num::NonZeroU64};
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
}

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
#[derive(Debug)]
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
    Decide(DecideError),
    /// The command or replay could not evolve state from an event.
    Evolve(EvolveError),
    /// Snapshot loading failed before replaying stream history.
    ReadSnapshot(ReadSnapshotError),
    /// Stream history loading failed.
    ReadStream(ReadStreamError),
    /// Appending the decided events failed after the command was accepted.
    Append(AppendStreamError),
    /// A decided domain event could not provide its stored event type.
    EventType(EventTypeError),
    /// A decided domain event could not encode its payload.
    EventEncode(PayloadEncodeError),
    /// A stored event could not be converted back into a domain event.
    DecodeEvent(DecodeError),
    /// The loaded snapshot claims a position newer than the stream can prove.
    SnapshotAheadOfStream(SnapshotAheadOfStream),
    /// The snapshot's recorded position cannot be advanced (u64 overflow).
    ReadAfterOverflow(ReadAfterOverflow),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotAheadOfStream {
    pub snapshot_position: StreamPosition,
    pub stream_position: Option<StreamPosition>,
}

enum ReplayStreamError<EvolveError, DecodeError> {
    Evolve(EvolveError),
    DecodeEvent(DecodeError),
}

enum AppendDecisionError<DecideError, EvolveError, AppendStreamError, EventTypeError, PayloadEncodeError> {
    Decide(DecideError),
    Evolve(EvolveError),
    Append(AppendStreamError),
    EventType(EventTypeError),
    EventEncode(PayloadEncodeError),
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

impl<
    DecideError,
    EvolveError,
    ReadSnapshotError,
    ReadStreamError,
    AppendStreamError,
    EventTypeError,
    PayloadEncodeError,
    DecodeError,
> std::fmt::Display
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
where
    DecideError: std::fmt::Display,
    EvolveError: std::fmt::Display,
    ReadSnapshotError: std::fmt::Display,
    ReadStreamError: std::fmt::Display,
    AppendStreamError: std::fmt::Display,
    EventTypeError: std::fmt::Display,
    PayloadEncodeError: std::fmt::Display,
    DecodeError: std::fmt::Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decide(source) => write!(f, "command decision failed: {source}"),
            Self::Evolve(source) => write!(f, "command state evolution failed: {source}"),
            Self::ReadSnapshot(source) => write!(f, "command snapshot read failed: {source}"),
            Self::ReadStream(source) => write!(f, "command stream read failed: {source}"),
            Self::Append(source) => write!(f, "command stream append failed: {source}"),
            Self::EventType(source) => write!(f, "command event type failed: {source}"),
            Self::EventEncode(source) => write!(f, "command event encoding failed: {source}"),
            Self::DecodeEvent(source) => write!(f, "command event decoding failed: {source}"),
            Self::SnapshotAheadOfStream(SnapshotAheadOfStream {
                snapshot_position,
                stream_position: Some(stream_position),
            }) => write!(
                f,
                "snapshot position {snapshot_position} is ahead of current stream position {stream_position}"
            ),
            Self::SnapshotAheadOfStream(SnapshotAheadOfStream {
                snapshot_position,
                stream_position: None,
            }) => write!(
                f,
                "snapshot position {snapshot_position} exists but the stream has no current position"
            ),
            Self::ReadAfterOverflow(source) => write!(f, "{source}"),
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
> std::error::Error
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
where
    DecideError: std::error::Error + 'static,
    EvolveError: std::error::Error + 'static,
    ReadSnapshotError: std::error::Error + 'static,
    ReadStreamError: std::error::Error + 'static,
    AppendStreamError: std::error::Error + 'static,
    EventTypeError: std::error::Error + 'static,
    PayloadEncodeError: std::error::Error + 'static,
    DecodeError: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decide(source) => Some(source),
            Self::Evolve(source) => Some(source),
            Self::ReadSnapshot(source) => Some(source),
            Self::ReadStream(source) => Some(source),
            Self::Append(source) => Some(source),
            Self::EventType(source) => Some(source),
            Self::EventEncode(source) => Some(source),
            Self::DecodeEvent(source) => Some(source),
            Self::SnapshotAheadOfStream(_) => None,
            Self::ReadAfterOverflow(source) => Some(source),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WithoutSnapshots;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WithoutSnapshotTaskScheduler;

pub struct Snapshots<'a, S, P, Spawn = WithoutSnapshotTaskScheduler> {
    snapshot_store: &'a S,
    policy: P,
    schedule_snapshot_task: Spawn,
}

impl<'a, S, P> Snapshots<'a, S, P, WithoutSnapshotTaskScheduler> {
    pub fn new(snapshot_store: &'a S, policy: P) -> Self {
        Self {
            snapshot_store,
            policy,
            schedule_snapshot_task: WithoutSnapshotTaskScheduler,
        }
    }
}

impl<'a, S, P, Spawn> Snapshots<'a, S, P, Spawn> {
    fn schedule_snapshot_tasks_with<NextSpawn>(
        self,
        schedule_snapshot_task: NextSpawn,
    ) -> Snapshots<'a, S, P, NextSpawn> {
        Snapshots {
            snapshot_store: self.snapshot_store,
            policy: self.policy,
            schedule_snapshot_task,
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

    fn into_snapshots(self) -> Snapshots<'a, Self::Store, Self::Policy, Self::SnapshotTaskScheduler>;
}

impl<'a, C, S, P, Spawn> IntoSnapshots<'a, C> for Snapshots<'a, S, P, Spawn>
where
    C: Decider,
{
    type Store = S;
    type Policy = P;
    type SnapshotTaskScheduler = Spawn;

    fn into_snapshots(self) -> Snapshots<'a, Self::Store, Self::Policy, Self::SnapshotTaskScheduler> {
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

    fn into_snapshots(self) -> Snapshots<'a, Self::Store, Self::Policy, Self::SnapshotTaskScheduler> {
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

impl<'a, E, S, C, P, Spawn, G> CommandExecution<'a, E, C, Snapshots<'a, S, P, Spawn>, G> {
    pub fn with_task_runtime<NextSpawn>(
        self,
        schedule_snapshot_task: NextSpawn,
    ) -> CommandExecution<'a, E, C, Snapshots<'a, S, P, NextSpawn>, G> {
        CommandExecution {
            event_store: self.event_store,
            command: self.command,
            write_precondition: self.write_precondition,
            snapshots: self.snapshots.schedule_snapshot_tasks_with(schedule_snapshot_task),
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

impl<E, S, C, P, Spawn, G> CommandExecution<'_, E, C, Snapshots<'_, S, P, Spawn>, G>
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
    G: NowV7,
    CommandWriteSnapshotError<S, C>: std::fmt::Display + Send + 'static,
    CommandEventTypeError<C>: std::error::Error + Send + Sync + 'static,
    CommandEventPayloadEncodeError<C>: std::error::Error + Send + Sync + 'static,
    CommandEventDecodeError<C>: std::error::Error + Send + Sync + 'static,
    C::State: SnapshotType,
{
    pub async fn execute(self) -> CommandWithSnapshotsResult<E, S, C> {
        let stream_id = self.command.stream_id();
        let snapshot = self
            .snapshots
            .snapshot_store
            .read_snapshot(ReadSnapshotRequest { stream_id })
            .await
            .map_err(CommandError::ReadSnapshot)?;
        let snapshot = snapshot.snapshot;
        let snapshot_position = snapshot.as_ref().map(|snapshot| snapshot.position);
        let state = snapshot
            .map(|snapshot| snapshot.payload)
            .unwrap_or_else(C::initial_state);
        let from = match snapshot_position {
            Some(position) => ReadFrom::after(position).map_err(CommandError::ReadAfterOverflow)?,
            None => ReadFrom::Beginning,
        };
        let stream_read = self
            .event_store
            .read_stream(ReadStreamRequest { stream_id, from })
            .await
            .map_err(CommandError::ReadStream)?;
        let current_position = stream_read.current_position;

        if let Some(snapshot_position) = snapshot_position {
            match current_position {
                Some(stream_position) if snapshot_position <= stream_position => {}
                stream_position => {
                    return Err(CommandError::SnapshotAheadOfStream(SnapshotAheadOfStream {
                        snapshot_position,
                        stream_position,
                    }));
                }
            }
        }

        let state = evolve_state_from_stream_events::<C>(state, &stream_read.events)?;
        let (append_outcome, events, state) = self.append_decision(current_position, stream_id, state).await?;
        let replayed_event_count = stream_read.events.len() as u64;

        // Keep the policy decision inline: for frequency policies it is cheaper
        // than spawning, and only the storage mutation needs to be best-effort.
        let snapshot_decision = self.snapshots.policy.decide_snapshot(DecideSnapshot {
            command: self.command,
            stream_position: append_outcome.stream_position,
            snapshot_position,
            state: &state,
            events: &events,
            replayed_event_count,
        });

        if snapshot_decision == SnapshotDecision::Take {
            schedule_snapshot_write(
                &self.snapshots.schedule_snapshot_task,
                self.snapshots.snapshot_store,
                stream_id,
                Snapshot::new(append_outcome.stream_position, state.clone()),
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
                stream_id: stream_id.borrow(),
                snapshot,
            })
            .await
        {
            tracing::warn!(stream_id = %stream_id_for_log, error = %source, "failed to write snapshot");
        }
    });
}

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
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use chrono::{DateTime, Utc};
    use futures::executor::block_on;
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;

    use super::*;
    use crate::{
        Decision, EventData, EventDecode, EventDecodeOutcome, EventEncode, EventIdentity, EventType,
        ReadSnapshotResponse, ReadStreamResponse, SnapshotType, StreamEvent, WriteSnapshotResponse,
    };

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).expect("test stream position must be non-zero")
    }

    fn encode_event<E, G>(event: &E, event_id_generator: &G, headers: &Headers) -> Event
    where
        E: EventType + EventIdentity + EventEncode,
        G: NowV7 + ?Sized,
        <E as EventType>::Error: std::fmt::Debug,
        <E as EventEncode>::Error: std::fmt::Debug,
    {
        let id = event
            .event_id()
            .unwrap_or_else(|| EventId::new(event_id_generator.now_v7()));
        Event {
            id,
            r#type: event.event_type().unwrap().to_string(),
            content: event.encode().unwrap(),
            headers: headers.clone(),
        }
    }

    #[derive(Debug, Clone)]
    struct TestCommand {
        id: String,
        action: TestAction,
        stream_id_calls: Arc<AtomicUsize>,
    }

    #[derive(Debug, Clone)]
    struct RequiredRegisterCommand {
        id: String,
        stream_id_calls: Arc<AtomicUsize>,
    }

    #[derive(Debug, Clone, Copy)]
    struct FixedUuidGenerator(Uuid);

    impl NowV7 for FixedUuidGenerator {
        fn now_v7(&self) -> Uuid {
            self.0
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestAction {
        Register,
        RegisterThenDisable,
        RegisterThenFail,
        RegisterThenBroken,
        Disable,
        Remove,
        EmitBroken,
        EmitUntyped,
        EmitUnencodable,
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    enum TestState {
        Missing,
        Present { enabled: bool },
    }

    impl SnapshotType for TestState {
        const SNAPSHOT_STREAM_PREFIX: &'static str = "test.command.v1.";
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum TestEvent {
        Registered { id: String },
        StateChanged { id: String, enabled: bool },
        Removed { id: String },
        Broken { id: String },
        Untyped { id: String },
        Unencodable { id: String },
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestDecisionError {
        AlreadyRegistered,
        Missing,
        AlreadyDisabled,
    }

    impl std::fmt::Display for TestDecisionError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{self:?}")
        }
    }

    impl std::error::Error for TestDecisionError {}

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestCommandError {
        BrokenEvent,
        AlreadyRegistered,
        Missing,
        AlreadyDisabled,
    }

    impl std::fmt::Display for TestCommandError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{self:?}")
        }
    }

    impl std::error::Error for TestCommandError {}

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestInfraError {
        ReadSnapshot,
        WriteSnapshot,
        ReadStream,
        Append,
        Json,
        EventType,
        EventEncode,
    }

    impl From<serde_json::Error> for TestInfraError {
        fn from(_value: serde_json::Error) -> Self {
            Self::Json
        }
    }

    impl std::fmt::Display for TestInfraError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{self:?}")
        }
    }

    impl std::error::Error for TestInfraError {}

    impl From<TestDecisionError> for TestCommandError {
        fn from(value: TestDecisionError) -> Self {
            match value {
                TestDecisionError::AlreadyRegistered => Self::AlreadyRegistered,
                TestDecisionError::Missing => Self::Missing,
                TestDecisionError::AlreadyDisabled => Self::AlreadyDisabled,
            }
        }
    }

    #[derive(Debug, Clone)]
    struct FakeRuntime {
        snapshot: Option<Snapshot<TestState>>,
        current_position: Option<StreamPosition>,
        stream_events: Vec<StreamEvent>,
        stream_position: StreamPosition,
        fail_read_snapshot: bool,
        fail_write_snapshot: bool,
        fail_read_stream: bool,
        fail_append: bool,
        loaded_stream_ids: Arc<Mutex<Vec<String>>>,
        reads_from: Arc<Mutex<Vec<ReadFrom>>>,
        stream_write_preconditions: Arc<Mutex<Vec<StreamWritePrecondition>>>,
        appended_events: Arc<Mutex<Vec<Event>>>,
        written_snapshots: Arc<Mutex<Vec<Snapshot<TestState>>>>,
    }

    impl Default for FakeRuntime {
        fn default() -> Self {
            Self {
                snapshot: None,
                current_position: None,
                stream_events: Vec::new(),
                stream_position: position(1),
                fail_read_snapshot: false,
                fail_write_snapshot: false,
                fail_read_stream: false,
                fail_append: false,
                loaded_stream_ids: Arc::new(Mutex::new(Vec::new())),
                reads_from: Arc::new(Mutex::new(Vec::new())),
                stream_write_preconditions: Arc::new(Mutex::new(Vec::new())),
                appended_events: Arc::new(Mutex::new(Vec::new())),
                written_snapshots: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl TestCommand {
        fn new(id: &str, action: TestAction) -> Self {
            Self {
                id: id.to_string(),
                action,
                stream_id_calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn stream_id_calls(&self) -> usize {
            self.stream_id_calls.load(Ordering::SeqCst)
        }
    }

    impl RequiredRegisterCommand {
        fn new(id: &str) -> Self {
            Self {
                id: id.to_string(),
                stream_id_calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    fn initial_test_state() -> TestState {
        TestState::Missing
    }

    fn evolve_test_state(_state: TestState, event: &TestEvent) -> Result<TestState, TestCommandError> {
        match event {
            TestEvent::Registered { .. } => Ok(TestState::Present { enabled: true }),
            TestEvent::StateChanged { enabled, .. } => Ok(TestState::Present { enabled: *enabled }),
            TestEvent::Removed { .. } => Ok(TestState::Missing),
            TestEvent::Broken { .. } => Err(TestCommandError::BrokenEvent),
            TestEvent::Untyped { .. } | TestEvent::Unencodable { .. } => Ok(TestState::Present { enabled: true }),
        }
    }

    impl Decider for TestCommand {
        type StreamId = str;
        type State = TestState;
        type Event = TestEvent;
        type DecideError = TestDecisionError;
        type EvolveError = TestCommandError;

        fn stream_id(&self) -> &Self::StreamId {
            self.stream_id_calls.fetch_add(1, Ordering::SeqCst);
            &self.id
        }

        fn initial_state() -> Self::State {
            initial_test_state()
        }

        fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
            evolve_test_state(state, event)
        }

        fn decide(state: &TestState, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
            match (state, command.action) {
                (TestState::Missing, TestAction::Register) => {
                    Ok(Decision::event(TestEvent::Registered { id: command.id.clone() }))
                }
                (TestState::Missing, TestAction::RegisterThenDisable) => Decision::<Self>::act()
                    .execute(|_, command: &Self| Decision::event(TestEvent::Registered { id: command.id.clone() }))
                    .execute(|state, command: &Self| {
                        assert_eq!(state, &TestState::Present { enabled: true });
                        Decision::event(TestEvent::StateChanged {
                            id: command.id.clone(),
                            enabled: false,
                        })
                    })
                    .into(),
                (TestState::Missing, TestAction::RegisterThenFail) => Decision::<Self>::act()
                    .execute(|_, command: &Self| Decision::event(TestEvent::Registered { id: command.id.clone() }))
                    .execute(|_, _| Err(TestDecisionError::AlreadyDisabled))
                    .into(),
                (TestState::Missing, TestAction::RegisterThenBroken) => Decision::<Self>::act()
                    .execute(|_, command: &Self| Decision::event(TestEvent::Registered { id: command.id.clone() }))
                    .execute(|_, command: &Self| Decision::event(TestEvent::Broken { id: command.id.clone() }))
                    .into(),
                (_, TestAction::EmitBroken) => Ok(Decision::event(TestEvent::Broken { id: command.id.clone() })),
                (_, TestAction::EmitUntyped) => Ok(Decision::event(TestEvent::Untyped { id: command.id.clone() })),
                (_, TestAction::EmitUnencodable) => {
                    Ok(Decision::event(TestEvent::Unencodable { id: command.id.clone() }))
                }
                (
                    TestState::Present { .. },
                    TestAction::Register
                    | TestAction::RegisterThenDisable
                    | TestAction::RegisterThenFail
                    | TestAction::RegisterThenBroken,
                ) => Err(TestDecisionError::AlreadyRegistered),
                (TestState::Present { enabled: false }, TestAction::Disable) => Err(TestDecisionError::AlreadyDisabled),
                (TestState::Present { .. }, TestAction::Disable) => Ok(Decision::event(TestEvent::StateChanged {
                    id: command.id.clone(),
                    enabled: false,
                })),
                (TestState::Missing, TestAction::Disable | TestAction::Remove) => Err(TestDecisionError::Missing),
                (TestState::Present { .. }, TestAction::Remove) => {
                    Ok(Decision::event(TestEvent::Removed { id: command.id.clone() }))
                }
            }
        }
    }

    impl CommandSnapshotPolicy for TestCommand {
        type SnapshotPolicy = NoSnapshot;

        const SNAPSHOT_POLICY: Self::SnapshotPolicy = NoSnapshot;
    }

    impl Decider for RequiredRegisterCommand {
        type StreamId = str;
        type State = TestState;
        type Event = TestEvent;
        type DecideError = TestDecisionError;
        type EvolveError = TestCommandError;

        const WRITE_PRECONDITION: Option<WritePrecondition> = Some(WritePrecondition::NoStream);

        fn stream_id(&self) -> &Self::StreamId {
            self.stream_id_calls.fetch_add(1, Ordering::SeqCst);
            &self.id
        }

        fn initial_state() -> Self::State {
            initial_test_state()
        }

        fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
            evolve_test_state(state, event)
        }

        fn decide(state: &TestState, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
            match state {
                TestState::Missing => Ok(Decision::event(TestEvent::Registered { id: command.id.clone() })),
                TestState::Present { .. } => Err(TestDecisionError::AlreadyRegistered),
            }
        }
    }

    impl EventIdentity for TestEvent {}

    impl EventType for TestEvent {
        type Error = TestInfraError;

        fn event_type(&self) -> Result<&'static str, Self::Error> {
            let event_type = match self {
                Self::Registered { .. } => "registered",
                Self::StateChanged { .. } => "state_changed",
                Self::Removed { .. } => "removed",
                Self::Broken { .. } => "broken",
                Self::Untyped { .. } => return Err(TestInfraError::EventType),
                Self::Unencodable { .. } => "unencodable",
            };
            Ok(event_type)
        }
    }

    impl EventEncode for TestEvent {
        type Error = TestInfraError;

        fn encode(&self) -> Result<Vec<u8>, Self::Error> {
            if matches!(self, Self::Unencodable { .. }) {
                return Err(TestInfraError::EventEncode);
            }
            serde_json::to_vec(self).map_err(Into::into)
        }
    }

    impl EventDecode for TestEvent {
        type Error = serde_json::Error;

        fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
            if event.event_type == "ignored" {
                return Ok(EventDecodeOutcome::Skipped);
            }

            serde_json::from_slice(event.payload).map(EventDecodeOutcome::Decoded)
        }
    }

    #[derive(Debug, Clone, Default)]
    struct RecordSnapshotPosition(Arc<Mutex<Option<Option<StreamPosition>>>>);

    impl<C: Decider> SnapshotPolicy<C> for RecordSnapshotPosition {
        fn decide_snapshot(&self, context: DecideSnapshot<'_, C>) -> SnapshotDecision {
            *self.0.lock().unwrap() = Some(context.snapshot_position);
            SnapshotDecision::Skip
        }
    }

    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    struct SnapshotOnDisable;

    impl SnapshotPolicy<TestCommand> for SnapshotOnDisable {
        fn decide_snapshot(&self, context: DecideSnapshot<'_, TestCommand>) -> SnapshotDecision {
            if matches!(context.command.action, TestAction::Disable) {
                SnapshotDecision::Take
            } else {
                SnapshotDecision::Skip
            }
        }
    }

    fn test_snapshots<P>(
        runtime: &FakeRuntime,
        policy: P,
    ) -> Snapshots<'_, FakeRuntime, P, ImmediateSnapshotTaskScheduler> {
        Snapshots::new(runtime, policy).schedule_snapshot_tasks_with(ImmediateSnapshotTaskScheduler)
    }

    impl StreamRead<str> for FakeRuntime {
        type Error = TestInfraError;

        async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
            if self.fail_read_stream {
                return Err(TestInfraError::ReadStream);
            }
            self.reads_from.lock().unwrap().push(request.from);
            let from_sequence = match request.from {
                ReadFrom::Beginning => 1,
                ReadFrom::Position(position) => position.as_u64(),
            };
            Ok(ReadStreamResponse {
                current_position: self.current_position,
                events: self
                    .stream_events
                    .iter()
                    .filter(|event| event.stream_position.as_u64() >= from_sequence)
                    .cloned()
                    .collect(),
            })
        }
    }

    impl StreamAppend<str> for FakeRuntime {
        type Error = TestInfraError;

        async fn append_stream(
            &self,
            request: AppendStreamRequest<'_, str>,
        ) -> Result<AppendStreamResponse, Self::Error> {
            if self.fail_append {
                return Err(TestInfraError::Append);
            }
            self.stream_write_preconditions
                .lock()
                .unwrap()
                .push(request.stream_write_precondition);
            self.appended_events.lock().unwrap().extend(request.events);
            Ok(AppendStreamResponse {
                stream_position: self.stream_position,
            })
        }
    }

    impl SnapshotRead<TestState, str> for FakeRuntime {
        type Error = TestInfraError;

        async fn read_snapshot(
            &self,
            request: ReadSnapshotRequest<'_, str>,
        ) -> Result<ReadSnapshotResponse<TestState>, Self::Error> {
            if self.fail_read_snapshot {
                return Err(TestInfraError::ReadSnapshot);
            }
            self.loaded_stream_ids
                .lock()
                .unwrap()
                .push(request.stream_id.to_string());
            Ok(ReadSnapshotResponse {
                snapshot: self.snapshot.clone(),
            })
        }
    }

    impl SnapshotWrite<TestState, str> for FakeRuntime {
        type Error = TestInfraError;

        async fn write_snapshot(
            &self,
            request: WriteSnapshotRequest<'_, TestState, str>,
        ) -> Result<WriteSnapshotResponse, Self::Error> {
            if self.fail_write_snapshot {
                return Err(TestInfraError::WriteSnapshot);
            }
            self.written_snapshots.lock().unwrap().push(request.snapshot);
            Ok(WriteSnapshotResponse)
        }
    }

    fn stream_event(sequence: u64, event: TestEvent) -> StreamEvent {
        let stream_id = match &event {
            TestEvent::Registered { id }
            | TestEvent::StateChanged { id, .. }
            | TestEvent::Removed { id }
            | TestEvent::Broken { id }
            | TestEvent::Untyped { id }
            | TestEvent::Unencodable { id } => id.clone(),
        };
        StreamEvent {
            stream_id,
            event: encode_event(&event, &UuidV7Generator, &Headers::empty()),
            stream_position: position(sequence),
            recorded_at: DateTime::<Utc>::from_timestamp(1_700_000_000 + sequence as i64, 0).unwrap(),
        }
    }

    fn invalid_stream_event(sequence: u64) -> StreamEvent {
        let mut event = stream_event(
            sequence,
            TestEvent::Registered {
                id: "alpha".to_string(),
            },
        );
        event.event.content = b"not-json".to_vec();
        event
    }

    fn skipped_stream_event(sequence: u64) -> StreamEvent {
        let mut event = stream_event(
            sequence,
            TestEvent::Removed {
                id: "alpha".to_string(),
            },
        );
        event.event.r#type = "ignored".to_string();
        event
    }

    type TestExecutionError = CommandError<
        TestDecisionError,
        TestCommandError,
        TestInfraError,
        TestInfraError,
        TestInfraError,
        TestInfraError,
        TestInfraError,
        serde_json::Error,
    >;

    #[test]
    fn tokio_snapshot_task_scheduler_reports_missing_runtime() {
        TokioSnapshotTaskScheduler.schedule(async {});
    }

    #[tokio::test]
    async fn tokio_snapshot_task_scheduler_spawns_on_current_runtime() {
        let executed = Arc::new(AtomicBool::new(false));
        let task_executed = Arc::clone(&executed);

        TokioSnapshotTaskScheduler.schedule(async move {
            task_executed.store(true, Ordering::SeqCst);
        });
        tokio::task::yield_now().await;

        assert!(executed.load(Ordering::SeqCst));
    }

    #[test]
    fn immediate_snapshot_task_scheduler_catches_task_panic() {
        ImmediateSnapshotTaskScheduler.schedule(async {
            panic!("snapshot task failed");
        });
    }

    #[test]
    fn command_errors_preserve_display_and_sources() {
        let decode_error = serde_json::from_slice::<TestEvent>(b"not-json").unwrap_err();
        let read_after_overflow = ReadFrom::after(StreamPosition::try_new(u64::MAX).unwrap()).unwrap_err();
        let read_after_overflow_message = read_after_overflow.to_string();
        let cases: Vec<(TestExecutionError, String, bool)> = vec![
            (
                CommandError::Decide(TestDecisionError::Missing),
                "command decision failed: Missing".to_string(),
                true,
            ),
            (
                CommandError::Evolve(TestCommandError::Missing),
                "command state evolution failed: Missing".to_string(),
                true,
            ),
            (
                CommandError::ReadSnapshot(TestInfraError::ReadSnapshot),
                "command snapshot read failed: ReadSnapshot".to_string(),
                true,
            ),
            (
                CommandError::ReadStream(TestInfraError::ReadStream),
                "command stream read failed: ReadStream".to_string(),
                true,
            ),
            (
                CommandError::Append(TestInfraError::Append),
                "command stream append failed: Append".to_string(),
                true,
            ),
            (
                CommandError::EventType(TestInfraError::EventType),
                "command event type failed: EventType".to_string(),
                true,
            ),
            (
                CommandError::EventEncode(TestInfraError::EventEncode),
                "command event encoding failed: EventEncode".to_string(),
                true,
            ),
            (
                CommandError::DecodeEvent(decode_error),
                "command event decoding failed: expected ident at line 1 column 2".to_string(),
                true,
            ),
            (
                CommandError::SnapshotAheadOfStream(SnapshotAheadOfStream {
                    snapshot_position: position(3),
                    stream_position: Some(position(2)),
                }),
                "snapshot position 3 is ahead of current stream position 2".to_string(),
                false,
            ),
            (
                CommandError::SnapshotAheadOfStream(SnapshotAheadOfStream {
                    snapshot_position: position(1),
                    stream_position: None,
                }),
                "snapshot position 1 exists but the stream has no current position".to_string(),
                false,
            ),
            (
                CommandError::ReadAfterOverflow(read_after_overflow),
                read_after_overflow_message,
                true,
            ),
        ];

        for (error, message, has_source) in cases {
            assert_eq!(error.to_string(), message);
            assert_eq!(std::error::Error::source(&error).is_some(), has_source);
        }
    }

    #[test]
    fn test_error_helpers_cover_all_variants() {
        assert_eq!(TestDecisionError::AlreadyRegistered.to_string(), "AlreadyRegistered");
        assert_eq!(TestDecisionError::Missing.to_string(), "Missing");
        assert_eq!(TestDecisionError::AlreadyDisabled.to_string(), "AlreadyDisabled");
        assert_eq!(TestCommandError::BrokenEvent.to_string(), "BrokenEvent");
        assert_eq!(TestInfraError::Json.to_string(), "Json");
        assert_eq!(
            TestInfraError::from(serde_json::from_slice::<serde_json::Value>(b"not-json").unwrap_err()),
            TestInfraError::Json
        );

        assert_eq!(
            TestCommandError::from(TestDecisionError::AlreadyRegistered),
            TestCommandError::AlreadyRegistered
        );
        assert_eq!(
            TestCommandError::from(TestDecisionError::Missing),
            TestCommandError::Missing
        );
        assert_eq!(
            TestCommandError::from(TestDecisionError::AlreadyDisabled),
            TestCommandError::AlreadyDisabled
        );
    }

    #[test]
    fn write_preconditions_convert_from_decider_metadata() {
        assert_eq!(
            StreamWritePrecondition::from(WritePrecondition::Any),
            StreamWritePrecondition::Any
        );
        assert_eq!(
            StreamWritePrecondition::from(WritePrecondition::StreamExists),
            StreamWritePrecondition::StreamExists
        );
        assert_eq!(
            StreamWritePrecondition::from(WritePrecondition::NoStream),
            StreamWritePrecondition::NoStream
        );
    }

    #[test]
    fn stream_event_helper_extracts_ids_for_edge_variants() {
        assert_eq!(
            stream_event(
                1,
                TestEvent::Removed {
                    id: "removed".to_string(),
                },
            )
            .stream_id,
            "removed"
        );
        assert!(
            std::panic::catch_unwind(|| {
                stream_event(
                    2,
                    TestEvent::Untyped {
                        id: "untyped".to_string(),
                    },
                );
            })
            .is_err()
        );
    }

    #[test]
    fn executes_from_initial_state_without_snapshot_or_history() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let result = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap();

        assert_eq!(result.stream_position, position(1));
        assert_eq!(result.state, TestState::Present { enabled: true });
        assert_eq!(
            result.events,
            Events::one(TestEvent::Registered {
                id: "alpha".to_string(),
            })
        );
        assert_eq!(
            runtime.stream_write_preconditions.lock().unwrap().as_slice(),
            &[StreamWritePrecondition::NoStream]
        );
        assert_eq!(runtime.reads_from.lock().unwrap().as_slice(), &[ReadFrom::Beginning]);
    }

    #[test]
    fn executes_act_decisions_with_evolved_step_state() {
        let runtime = FakeRuntime {
            stream_position: position(2),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::RegisterThenDisable);

        let result = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap();

        assert_eq!(result.stream_position, position(2));
        assert_eq!(result.state, TestState::Present { enabled: false });
        assert_eq!(
            result.events,
            Events::from_vec(vec![
                TestEvent::Registered {
                    id: "alpha".to_string(),
                },
                TestEvent::StateChanged {
                    id: "alpha".to_string(),
                    enabled: false,
                },
            ])
            .unwrap()
        );
        assert_eq!(
            runtime.stream_write_preconditions.lock().unwrap().as_slice(),
            &[StreamWritePrecondition::NoStream]
        );
        assert_eq!(runtime.appended_events.lock().unwrap().len(), 2);
    }

    #[test]
    fn restores_from_snapshot_and_reads_only_delta_after_snapshot_position() {
        let runtime = FakeRuntime {
            snapshot: Some(Snapshot::new(position(1), TestState::Present { enabled: true })),
            current_position: Some(position(2)),
            stream_events: vec![
                stream_event(
                    1,
                    TestEvent::Registered {
                        id: "alpha".to_string(),
                    },
                ),
                stream_event(
                    2,
                    TestEvent::StateChanged {
                        id: "alpha".to_string(),
                        enabled: false,
                    },
                ),
            ],
            stream_position: position(3),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Remove);

        let result = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(test_snapshots(&runtime, NoSnapshot))
                .execute(),
        )
        .unwrap();

        assert_eq!(
            runtime.reads_from.lock().unwrap().as_slice(),
            &[ReadFrom::Position(position(2))]
        );
        assert_eq!(
            runtime.stream_write_preconditions.lock().unwrap().as_slice(),
            &[StreamWritePrecondition::At(position(2))]
        );
        assert_eq!(result.state, TestState::Missing);
    }

    #[test]
    fn command_snapshot_policy_allows_store_shorthand() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let result = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(&runtime)
                .with_task_runtime(ImmediateSnapshotTaskScheduler)
                .execute(),
        )
        .unwrap();

        assert_eq!(result.state, TestState::Present { enabled: true });
        assert_eq!(
            runtime.loaded_stream_ids.lock().unwrap().as_slice(),
            &["alpha".to_string()]
        );
        assert!(runtime.written_snapshots.lock().unwrap().is_empty());
    }

    #[test]
    fn errors_when_snapshot_is_ahead_of_stream() {
        let runtime = FakeRuntime {
            snapshot: Some(Snapshot::new(position(3), TestState::Present { enabled: true })),
            current_position: Some(position(2)),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Remove);

        let error = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(test_snapshots(&runtime, NoSnapshot))
                .execute(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CommandError::SnapshotAheadOfStream(SnapshotAheadOfStream {
                snapshot_position,
                stream_position: Some(stream_position),
            }) if snapshot_position == position(3) && stream_position == position(2)
        ));
    }

    #[test]
    fn errors_when_snapshot_exists_without_stream_history() {
        let runtime = FakeRuntime {
            snapshot: Some(Snapshot::new(position(1), TestState::Present { enabled: true })),
            current_position: None,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Remove);

        let error = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(test_snapshots(&runtime, NoSnapshot))
                .execute(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CommandError::SnapshotAheadOfStream(SnapshotAheadOfStream {
                snapshot_position,
                stream_position: None,
            }) if snapshot_position == position(1)
        ));
    }

    #[test]
    fn propagates_replay_evolve_failures() {
        let runtime = FakeRuntime {
            current_position: Some(position(1)),
            stream_events: vec![stream_event(
                1,
                TestEvent::Broken {
                    id: "alpha".to_string(),
                },
            )],
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let error = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap_err();

        assert!(matches!(error, CommandError::Evolve(TestCommandError::BrokenEvent)));
    }

    #[test]
    fn propagates_stream_decode_failures() {
        let runtime = FakeRuntime {
            current_position: Some(position(1)),
            stream_events: vec![invalid_stream_event(1)],
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let error = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap_err();

        assert!(matches!(error, CommandError::DecodeEvent(_)));
    }

    #[test]
    fn does_not_append_events_that_cannot_evolve() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::EmitBroken);

        let error = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap_err();

        assert!(matches!(error, CommandError::Evolve(TestCommandError::BrokenEvent)));
        assert!(runtime.stream_write_preconditions.lock().unwrap().is_empty());
        assert!(runtime.appended_events.lock().unwrap().is_empty());
    }

    #[test]
    fn propagates_event_type_failures_without_append() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::EmitUntyped);

        let error = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap_err();

        assert!(matches!(error, CommandError::EventType(TestInfraError::EventType)));
        assert!(runtime.stream_write_preconditions.lock().unwrap().is_empty());
        assert!(runtime.appended_events.lock().unwrap().is_empty());
    }

    #[test]
    fn propagates_event_encode_failures_without_append() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::EmitUnencodable);

        let error = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap_err();

        assert!(matches!(error, CommandError::EventEncode(TestInfraError::EventEncode)));
        assert!(runtime.stream_write_preconditions.lock().unwrap().is_empty());
        assert!(runtime.appended_events.lock().unwrap().is_empty());
    }

    #[test]
    fn propagates_act_decide_failures_without_append() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::RegisterThenFail);

        let error = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap_err();

        assert!(matches!(
            error,
            CommandError::Decide(TestDecisionError::AlreadyDisabled)
        ));
        assert!(runtime.stream_write_preconditions.lock().unwrap().is_empty());
        assert!(runtime.appended_events.lock().unwrap().is_empty());
    }

    #[test]
    fn propagates_act_evolve_failures_without_append() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::RegisterThenBroken);

        let error = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap_err();

        assert!(matches!(error, CommandError::Evolve(TestCommandError::BrokenEvent)));
        assert!(runtime.stream_write_preconditions.lock().unwrap().is_empty());
        assert!(runtime.appended_events.lock().unwrap().is_empty());
    }

    #[test]
    fn propagates_decision_failures() {
        let runtime = FakeRuntime {
            snapshot: Some(Snapshot::new(position(1), TestState::Present { enabled: true })),
            current_position: Some(position(1)),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let error = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(test_snapshots(&runtime, NoSnapshot))
                .execute(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CommandError::Decide(TestDecisionError::AlreadyRegistered)
        ));
    }

    #[test]
    fn propagates_missing_decision_failures() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Disable);

        let error = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap_err();

        assert!(matches!(error, CommandError::Decide(TestDecisionError::Missing)));
    }

    #[test]
    fn propagates_already_disabled_decision_failures() {
        let runtime = FakeRuntime {
            snapshot: Some(Snapshot::new(position(1), TestState::Present { enabled: false })),
            current_position: Some(position(1)),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Disable);

        let error = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(test_snapshots(&runtime, NoSnapshot))
                .execute(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CommandError::Decide(TestDecisionError::AlreadyDisabled)
        ));
    }

    #[test]
    fn required_register_rejects_existing_state() {
        let runtime = FakeRuntime {
            current_position: Some(position(1)),
            stream_events: vec![stream_event(
                1,
                TestEvent::Registered {
                    id: "alpha".to_string(),
                },
            )],
            stream_position: position(2),
            ..Default::default()
        };
        let command = RequiredRegisterCommand::new("alpha");

        let error = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap_err();

        assert!(matches!(
            error,
            CommandError::Decide(TestDecisionError::AlreadyRegistered)
        ));
    }

    #[test]
    fn encodes_emitted_events_and_returns_stream_position() {
        let runtime = FakeRuntime {
            snapshot: Some(Snapshot::new(position(1), TestState::Present { enabled: true })),
            current_position: Some(position(1)),
            stream_position: position(2),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Disable);

        let result = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(test_snapshots(&runtime, NoSnapshot))
                .execute(),
        )
        .unwrap();
        let appended_events = runtime.appended_events.lock().unwrap();

        assert_eq!(result.stream_position, position(2));
        assert_eq!(
            runtime.stream_write_preconditions.lock().unwrap().as_slice(),
            &[StreamWritePrecondition::At(position(1))]
        );
        assert_eq!(appended_events.len(), 1);
        assert_eq!(appended_events[0].r#type, "state_changed");
        assert_eq!(result.state, TestState::Present { enabled: false });
    }

    #[test]
    fn builder_applies_headers_and_event_id_generator_with_snapshots() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);
        let headers = Headers::from_entries([("trace-id", "trace-1")]).unwrap();
        let event_id = Uuid::from_u128(0x018d_0000_0000_7000_8000_0000_0000_0001);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(test_snapshots(&runtime, NoSnapshot))
                .with_headers(headers)
                .with_event_id_generator(FixedUuidGenerator(event_id))
                .execute(),
        )
        .unwrap();
        let appended_events = runtime.appended_events.lock().unwrap();

        assert_eq!(appended_events[0].headers.get_str("trace-id"), Some("trace-1"));
        assert_eq!(appended_events[0].id.as_uuid(), event_id);
    }

    #[test]
    fn falls_back_to_exact_current_position_when_command_has_no_required_rule() {
        let runtime = FakeRuntime {
            current_position: Some(position(7)),
            stream_events: vec![stream_event(
                7,
                TestEvent::Registered {
                    id: "alpha".to_string(),
                },
            )],
            stream_position: position(8),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Remove);

        let _ = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap();

        assert_eq!(
            runtime.stream_write_preconditions.lock().unwrap().as_slice(),
            &[StreamWritePrecondition::At(position(7))]
        );
    }

    #[test]
    fn replay_skips_events_outside_the_decider_event_set() {
        let runtime = FakeRuntime {
            current_position: Some(position(2)),
            stream_events: vec![
                skipped_stream_event(1),
                stream_event(
                    2,
                    TestEvent::Registered {
                        id: "alpha".to_string(),
                    },
                ),
            ],
            stream_position: position(3),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Disable);

        let result = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap();

        assert_eq!(result.state, TestState::Present { enabled: false });
        assert_eq!(
            runtime.stream_write_preconditions.lock().unwrap().as_slice(),
            &[StreamWritePrecondition::At(position(2))]
        );
        assert_eq!(runtime.appended_events.lock().unwrap()[0].r#type, "state_changed");
    }

    #[test]
    fn explicit_write_precondition_overrides_exact_current_position_fallback() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .with_write_precondition(StreamWritePrecondition::Any)
                .execute(),
        )
        .unwrap();

        assert_eq!(
            runtime.stream_write_preconditions.lock().unwrap().as_slice(),
            &[StreamWritePrecondition::Any]
        );
    }

    #[test]
    fn required_command_rule_uses_required_stream_write_precondition() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = RequiredRegisterCommand::new("alpha");

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .with_write_precondition(StreamWritePrecondition::Any)
                .execute(),
        )
        .unwrap();

        assert_eq!(
            runtime.stream_write_preconditions.lock().unwrap().as_slice(),
            &[StreamWritePrecondition::NoStream]
        );
    }

    #[test]
    fn propagates_stream_read_failures() {
        let runtime = FakeRuntime {
            fail_read_stream: true,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let error = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap_err();

        assert!(matches!(error, CommandError::ReadStream(TestInfraError::ReadStream)));
    }

    #[test]
    fn propagates_snapshot_read_failures() {
        let runtime = FakeRuntime {
            fail_read_snapshot: true,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let error = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(test_snapshots(&runtime, NoSnapshot))
                .execute(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CommandError::ReadSnapshot(TestInfraError::ReadSnapshot)
        ));
    }

    #[test]
    fn propagates_append_failures() {
        let runtime = FakeRuntime {
            fail_append: true,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let error = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap_err();

        assert!(matches!(error, CommandError::Append(TestInfraError::Append)));
    }

    #[test]
    fn writes_snapshot_when_policy_requests_it() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let result = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(Snapshots::new(&runtime, FrequencySnapshot::new(NonZeroU64::MIN)))
                .with_task_runtime(ImmediateSnapshotTaskScheduler)
                .execute(),
        )
        .unwrap();

        assert_eq!(result.state, TestState::Present { enabled: true });
        assert_eq!(
            runtime.written_snapshots.lock().unwrap().as_slice(),
            &[Snapshot::new(position(1), TestState::Present { enabled: true })]
        );
    }

    #[test]
    fn frequency_snapshot_writes_after_enough_replayed_and_emitted_events() {
        const EVERY_TWO_EVENTS: NonZeroU64 = NonZeroU64::new(2).expect("snapshot cadence must be non-zero");
        assert_eq!(FrequencySnapshot::new(EVERY_TWO_EVENTS).frequency(), EVERY_TWO_EVENTS);
        let runtime = FakeRuntime {
            current_position: Some(position(1)),
            stream_events: vec![stream_event(
                1,
                TestEvent::Registered {
                    id: "alpha".to_string(),
                },
            )],
            stream_position: position(3),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Disable);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(test_snapshots(&runtime, FrequencySnapshot::new(EVERY_TWO_EVENTS)))
                .execute(),
        )
        .unwrap();

        assert_eq!(
            runtime.written_snapshots.lock().unwrap().as_slice(),
            &[Snapshot::new(position(3), TestState::Present { enabled: false })]
        );
    }

    #[test]
    fn frequency_snapshot_skips_before_enough_replayed_and_emitted_events() {
        const EVERY_TWO_EVENTS: NonZeroU64 = NonZeroU64::new(2).expect("snapshot cadence must be non-zero");
        let runtime = FakeRuntime {
            stream_position: position(2),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(test_snapshots(&runtime, FrequencySnapshot::new(EVERY_TWO_EVENTS)))
                .execute(),
        )
        .unwrap();

        assert!(runtime.written_snapshots.lock().unwrap().is_empty());
    }

    #[test]
    fn does_not_write_snapshot_when_policy_skips_it() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(test_snapshots(&runtime, NoSnapshot))
                .execute(),
        )
        .unwrap();

        assert!(runtime.written_snapshots.lock().unwrap().is_empty());
    }

    #[test]
    fn does_not_fail_committed_command_when_snapshot_write_fails() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            fail_write_snapshot: true,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let result = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(test_snapshots(&runtime, FrequencySnapshot::new(NonZeroU64::MIN)))
                .execute(),
        )
        .unwrap();

        assert_eq!(result.stream_position, position(1));
        assert_eq!(result.state, TestState::Present { enabled: true });
        assert!(runtime.written_snapshots.lock().unwrap().is_empty());
    }

    #[test]
    fn command_aware_snapshot_policy_snapshots_based_on_command_action() {
        let runtime = FakeRuntime {
            current_position: Some(position(1)),
            stream_events: vec![stream_event(
                1,
                TestEvent::Registered {
                    id: "alpha".to_string(),
                },
            )],
            stream_position: position(2),
            ..Default::default()
        };
        let disable_command = TestCommand::new("alpha", TestAction::Disable);
        let register_command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(
            CommandExecution::new(&runtime, &disable_command)
                .with_snapshot(test_snapshots(&runtime, SnapshotOnDisable))
                .execute(),
        )
        .unwrap();

        assert_eq!(
            runtime.written_snapshots.lock().unwrap().as_slice(),
            &[Snapshot::new(position(2), TestState::Present { enabled: false })]
        );

        runtime.written_snapshots.lock().unwrap().clear();
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };

        let _ = block_on(
            CommandExecution::new(&runtime, &register_command)
                .with_snapshot(test_snapshots(&runtime, SnapshotOnDisable))
                .execute(),
        )
        .unwrap();

        assert!(runtime.written_snapshots.lock().unwrap().is_empty());
    }

    #[test]
    fn snapshot_position_is_none_without_loaded_snapshot() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let snapshot_position = Arc::new(Mutex::new(None));
        let command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(test_snapshots(
                    &runtime,
                    RecordSnapshotPosition(snapshot_position.clone()),
                ))
                .execute(),
        )
        .unwrap();

        assert_eq!(*snapshot_position.lock().unwrap(), Some(None));
    }

    #[test]
    fn snapshot_position_matches_loaded_snapshot_position() {
        let loaded_snapshot_position = position(2);
        let runtime = FakeRuntime {
            snapshot: Some(Snapshot::new(
                loaded_snapshot_position,
                TestState::Present { enabled: true },
            )),
            current_position: Some(position(3)),
            stream_events: vec![stream_event(
                3,
                TestEvent::StateChanged {
                    id: "alpha".to_string(),
                    enabled: true,
                },
            )],
            stream_position: position(4),
            ..Default::default()
        };
        let snapshot_position = Arc::new(Mutex::new(None));
        let command = TestCommand::new("alpha", TestAction::Disable);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(test_snapshots(
                    &runtime,
                    RecordSnapshotPosition(snapshot_position.clone()),
                ))
                .execute(),
        )
        .unwrap();

        assert_eq!(*snapshot_position.lock().unwrap(), Some(Some(loaded_snapshot_position)));
    }

    #[test]
    fn resolves_stream_id_once() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap();

        assert_eq!(command.stream_id_calls(), 1);
        assert!(runtime.loaded_stream_ids.lock().unwrap().is_empty());
    }
}
