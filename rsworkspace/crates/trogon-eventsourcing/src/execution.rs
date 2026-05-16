use crate::snapshot::{
    ReadSnapshotRequest, Snapshot, SnapshotRead, SnapshotSchema, SnapshotStoreConfig, SnapshotWrite,
    WriteSnapshotRequest,
};
use crate::stream::{
    AppendStreamRequest, AppendStreamResponse, ReadStreamRequest, StreamAppend, StreamPosition, StreamRead,
    StreamWritePrecondition,
};
use crate::{
    CanonicalEventCodec, Decider, Event, EventCodec, EventEncodeError, EventIdentity, EventType, Events, StreamEvent,
    WritePrecondition,
};
use trogon_decider::DecisionFailure;

use std::{borrow::Borrow, future::Future, num::NonZeroU64, pin::Pin};

type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type BoxTask = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
type ExecutionFailure<C, RuntimeError> =
    CommandFailure<<C as Decider>::DecideError, <C as Decider>::EvolveError, RuntimeError>;
type ExecutionStep<C, RuntimeError, T> = Result<T, ExecutionFailure<C, RuntimeError>>;

pub fn spawn_on_tokio(task: BoxTask) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("failed to schedule background task without an active Tokio runtime");
        return;
    };

    drop(handle.spawn(task));
}

pub fn run_task_immediately(task: BoxTask) {
    let handle = std::thread::spawn(move || futures::executor::block_on(task));
    if handle.join().is_err() {
        tracing::warn!("background task panicked");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotDecision {
    Skip,
    Take,
}

#[derive(Debug, Clone, Copy)]
pub struct SnapshotDecisionContext<'a, State, Event> {
    /// The stream high-watermark after the append that may trigger a snapshot.
    ///
    /// Use this as the checkpoint position if the policy decides to snapshot.
    /// Do not use it as a gapless event count.
    pub stream_position: StreamPosition,
    /// Number of events this execution applied since the loaded snapshot.
    ///
    /// Frequency-based policies use this value instead of `stream_position`
    /// because stream positions may be sparse.
    pub events_since_snapshot: u64,
    pub state: &'a State,
    pub events: &'a Events<Event>,
}

pub trait SnapshotPolicy<State, Event> {
    fn snapshot_decision(&self, context: SnapshotDecisionContext<'_, State, Event>) -> SnapshotDecision;
}

pub trait CommandSnapshotPolicy: Decider
where
    Self::State: SnapshotSchema,
{
    type SnapshotPolicy: SnapshotPolicy<Self::State, Self::Event>;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy;

    fn snapshots<'a, S>(snapshot_store: &'a S) -> Snapshots<'a, S, Self::SnapshotPolicy> {
        Snapshots::new(
            snapshot_store,
            Self::State::snapshot_store_config(),
            Self::SNAPSHOT_POLICY,
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoSnapshot;

impl<State, Event> SnapshotPolicy<State, Event> for NoSnapshot {
    fn snapshot_decision(&self, _context: SnapshotDecisionContext<'_, State, Event>) -> SnapshotDecision {
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

impl<State, Event> SnapshotPolicy<State, Event> for FrequencySnapshot {
    fn snapshot_decision(&self, context: SnapshotDecisionContext<'_, State, Event>) -> SnapshotDecision {
        if context.events_since_snapshot >= self.frequency.get() {
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
    pub events: Events<Event>,
    pub state: State,
}

pub type CommandResult<C, InfraError> = Result<
    ExecutionResult<<C as Decider>::State, <C as Decider>::Event>,
    CommandFailure<<C as Decider>::DecideError, <C as Decider>::EvolveError, InfraError>,
>;

#[derive(Debug)]
pub enum CommandFailure<DecideError, EvolveError, RuntimeError> {
    Decide(DecideError),
    Evolve(EvolveError),
    ReadSnapshot(RuntimeError),
    ReadStream(RuntimeError),
    Append(RuntimeError),
    EncodeEvent(BoxError),
    DecodeEvent(BoxError),
    SnapshotAheadOfStream {
        snapshot_position: StreamPosition,
        stream_position: Option<StreamPosition>,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WithoutSnapshots;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WithoutSnapshotTaskScheduler;

pub struct Snapshots<'a, S, P, Spawn = WithoutSnapshotTaskScheduler> {
    snapshot_store: &'a S,
    snapshot_config: SnapshotStoreConfig,
    policy: P,
    schedule_snapshot_task: Spawn,
}

impl<'a, S, P> Snapshots<'a, S, P, WithoutSnapshotTaskScheduler> {
    pub fn new(snapshot_store: &'a S, snapshot_config: SnapshotStoreConfig, policy: P) -> Self {
        Self {
            snapshot_store,
            snapshot_config,
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
            snapshot_config: self.snapshot_config,
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
    C::State: SnapshotSchema,
{
    type Store = S;
    type Policy = C::SnapshotPolicy;
    type SnapshotTaskScheduler = WithoutSnapshotTaskScheduler;

    fn into_snapshots(self) -> Snapshots<'a, Self::Store, Self::Policy, Self::SnapshotTaskScheduler> {
        C::snapshots(self)
    }
}

pub struct CommandExecution<'a, E, C, S = WithoutSnapshots> {
    event_store: &'a E,
    command: &'a C,
    write_precondition: Option<StreamWritePrecondition>,
    snapshots: S,
}

impl<'a, E, C> CommandExecution<'a, E, C, WithoutSnapshots>
where
    C: Decider,
{
    pub const fn new(event_store: &'a E, command: &'a C) -> Self {
        Self {
            event_store,
            command,
            write_precondition: None,
            snapshots: WithoutSnapshots,
        }
    }

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
    >
    where
        I: IntoSnapshots<'a, C>,
    {
        CommandExecution {
            event_store: self.event_store,
            command: self.command,
            write_precondition: self.write_precondition,
            snapshots: snapshots.into_snapshots(),
        }
    }
}

impl<'a, E, C, S> CommandExecution<'a, E, C, S> {
    pub fn with_write_precondition<W>(mut self, write_precondition: W) -> Self
    where
        W: Into<Option<StreamWritePrecondition>>,
    {
        self.write_precondition = write_precondition.into();
        self
    }
}

impl<'a, E, C, S> CommandExecution<'a, E, C, S> {
    pub fn with_codec<EC>(self, event_codec: EC) -> CommandExecutionWithCodec<'a, E, C, S, EC> {
        CommandExecutionWithCodec {
            event_store: self.event_store,
            command: self.command,
            write_precondition: self.write_precondition,
            snapshots: self.snapshots,
            event_codec,
        }
    }
}

pub struct CommandExecutionWithCodec<'a, E, C, S, EC> {
    event_store: &'a E,
    command: &'a C,
    write_precondition: Option<StreamWritePrecondition>,
    snapshots: S,
    event_codec: EC,
}

impl<'a, E, C, S, EC> CommandExecutionWithCodec<'a, E, C, S, EC>
where
    C: Decider,
{
    #[allow(clippy::type_complexity)]
    pub fn with_snapshot<I>(
        self,
        snapshots: I,
    ) -> CommandExecutionWithCodec<
        'a,
        E,
        C,
        Snapshots<
            'a,
            <I as IntoSnapshots<'a, C>>::Store,
            <I as IntoSnapshots<'a, C>>::Policy,
            <I as IntoSnapshots<'a, C>>::SnapshotTaskScheduler,
        >,
        EC,
    >
    where
        I: IntoSnapshots<'a, C>,
    {
        CommandExecutionWithCodec {
            event_store: self.event_store,
            command: self.command,
            write_precondition: self.write_precondition,
            snapshots: snapshots.into_snapshots(),
            event_codec: self.event_codec,
        }
    }
}

impl<'a, E, C, S, EC> CommandExecutionWithCodec<'a, E, C, S, EC> {
    pub fn with_write_precondition<W>(mut self, write_precondition: W) -> Self
    where
        W: Into<Option<StreamWritePrecondition>>,
    {
        self.write_precondition = write_precondition.into();
        self
    }

    async fn append_decision<SErr>(
        &self,
        current_position: Option<StreamPosition>,
        stream_id: &C::StreamId,
        state: C::State,
    ) -> ExecutionStep<C, SErr, (AppendStreamResponse, Events<C::Event>, C::State)>
    where
        C: Decider,
        C::Event: Clone + EventType + EventIdentity,
        C::StreamId: AsRef<str>,
        E: StreamAppend<C::StreamId, Error = SErr>,
        EC: EventCodec<C::Event>,
        <C::Event as EventType>::Error: std::error::Error + Send + Sync + 'static,
        EC::Error: std::error::Error + Send + Sync + 'static,
    {
        let decision = C::decide(&state, self.command).map_err(CommandFailure::Decide)?;
        let (state, events) = decision
            .handle(state, self.command)
            .map_err(decision_failure_to_command_failure::<C, SErr>)?;
        let encoded_events = encode_events(&self.event_codec, &events)
            .map_err(|source| CommandFailure::EncodeEvent(box_error(source)))?;
        let stream_write_precondition =
            resolve_stream_write_precondition::<C>(self.write_precondition, current_position);
        let append_outcome = self
            .event_store
            .append_stream(AppendStreamRequest::new(
                stream_id,
                stream_write_precondition,
                encoded_events,
            ))
            .await
            .map_err(|source| CommandFailure::Append(source))?;

        Ok((append_outcome, events, state))
    }
}

impl<'a, E, S, C, P, Spawn> CommandExecution<'a, E, C, Snapshots<'a, S, P, Spawn>> {
    pub fn with_task_runtime<NextSpawn>(
        self,
        schedule_snapshot_task: NextSpawn,
    ) -> CommandExecution<'a, E, C, Snapshots<'a, S, P, NextSpawn>> {
        CommandExecution {
            event_store: self.event_store,
            command: self.command,
            write_precondition: self.write_precondition,
            snapshots: self.snapshots.schedule_snapshot_tasks_with(schedule_snapshot_task),
        }
    }
}

impl<'a, E, S, C, P, Spawn, EC> CommandExecutionWithCodec<'a, E, C, Snapshots<'a, S, P, Spawn>, EC> {
    pub fn with_task_runtime<NextSpawn>(
        self,
        schedule_snapshot_task: NextSpawn,
    ) -> CommandExecutionWithCodec<'a, E, C, Snapshots<'a, S, P, NextSpawn>, EC> {
        CommandExecutionWithCodec {
            event_store: self.event_store,
            command: self.command,
            write_precondition: self.write_precondition,
            snapshots: self.snapshots.schedule_snapshot_tasks_with(schedule_snapshot_task),
            event_codec: self.event_codec,
        }
    }
}

impl<E, C, SErr> CommandExecution<'_, E, C, WithoutSnapshots>
where
    C: Decider,
    C::Event: Clone + CanonicalEventCodec + EventType + EventIdentity,
    C::StreamId: AsRef<str>,
    E: StreamRead<C::StreamId, Error = SErr> + StreamAppend<C::StreamId, Error = SErr>,
    <C::Event as CanonicalEventCodec>::Codec: EventCodec<C::Event>,
    <C::Event as EventType>::Error: std::error::Error + Send + Sync + 'static,
    <<C::Event as CanonicalEventCodec>::Codec as EventCodec<C::Event>>::Error:
        std::error::Error + Send + Sync + 'static,
{
    pub async fn execute(self) -> CommandResult<C, SErr> {
        self.execute_result().await
    }

    async fn execute_result(self) -> CommandResult<C, SErr> {
        self.with_codec(C::Event::canonical_codec()).execute_result().await
    }
}

impl<E, C, EC, SErr> CommandExecutionWithCodec<'_, E, C, WithoutSnapshots, EC>
where
    C: Decider,
    C::Event: Clone + EventType + EventIdentity,
    C::StreamId: AsRef<str>,
    E: StreamRead<C::StreamId, Error = SErr> + StreamAppend<C::StreamId, Error = SErr>,
    EC: EventCodec<C::Event>,
    <C::Event as EventType>::Error: std::error::Error + Send + Sync + 'static,
    EC::Error: std::error::Error + Send + Sync + 'static,
{
    pub async fn execute(self) -> CommandResult<C, SErr> {
        self.execute_result().await
    }

    async fn execute_result(self) -> CommandResult<C, SErr> {
        let stream_id = self.command.stream_id();
        let stream_read = self
            .event_store
            .read_stream(ReadStreamRequest::new(stream_id, 1))
            .await
            .map_err(|source| CommandFailure::ReadStream(source))?;
        let current_position = stream_read.current_position;
        let state = replay_stream_events::<C, EC, SErr>(C::initial_state(), stream_read.events, &self.event_codec)?;
        let (append_outcome, events, state) = self.append_decision::<SErr>(current_position, stream_id, state).await?;

        Ok(ExecutionResult {
            stream_position: append_outcome.stream_position,
            events,
            state,
        })
    }
}

impl<E, S, C, P, Spawn, SErr> CommandExecution<'_, E, C, Snapshots<'_, S, P, Spawn>>
where
    C: Decider,
    C::State: Clone + Send + 'static,
    C::Event: Clone + CanonicalEventCodec + EventType + EventIdentity,
    C::StreamId: AsRef<str> + ToOwned,
    <C::StreamId as ToOwned>::Owned: Borrow<C::StreamId> + Send + 'static,
    E: StreamRead<C::StreamId, Error = SErr> + StreamAppend<C::StreamId, Error = SErr>,
    S: Clone
        + SnapshotRead<C::State, C::StreamId, Error = SErr>
        + SnapshotWrite<C::State, C::StreamId, Error = SErr>
        + 'static,
    P: SnapshotPolicy<C::State, C::Event>,
    Spawn: Fn(BoxTask) + Send + Sync,
    SErr: std::fmt::Display + Send + 'static,
    <C::Event as CanonicalEventCodec>::Codec: EventCodec<C::Event>,
    <C::Event as EventType>::Error: std::error::Error + Send + Sync + 'static,
    <<C::Event as CanonicalEventCodec>::Codec as EventCodec<C::Event>>::Error:
        std::error::Error + Send + Sync + 'static,
{
    pub async fn execute(self) -> CommandResult<C, SErr> {
        self.execute_result().await
    }

    async fn execute_result(self) -> CommandResult<C, SErr> {
        self.with_codec(C::Event::canonical_codec()).execute_result().await
    }
}

impl<E, S, C, P, Spawn, SErr, EC> CommandExecutionWithCodec<'_, E, C, Snapshots<'_, S, P, Spawn>, EC>
where
    C: Decider,
    C::State: Clone + Send + 'static,
    C::Event: Clone + EventType + EventIdentity,
    C::StreamId: AsRef<str> + ToOwned,
    <C::StreamId as ToOwned>::Owned: Borrow<C::StreamId> + Send + 'static,
    E: StreamRead<C::StreamId, Error = SErr> + StreamAppend<C::StreamId, Error = SErr>,
    S: Clone
        + SnapshotRead<C::State, C::StreamId, Error = SErr>
        + SnapshotWrite<C::State, C::StreamId, Error = SErr>
        + 'static,
    P: SnapshotPolicy<C::State, C::Event>,
    Spawn: Fn(BoxTask) + Send + Sync,
    SErr: std::fmt::Display + Send + 'static,
    EC: EventCodec<C::Event>,
    <C::Event as EventType>::Error: std::error::Error + Send + Sync + 'static,
    EC::Error: std::error::Error + Send + Sync + 'static,
{
    pub async fn execute(self) -> CommandResult<C, SErr> {
        self.execute_result().await
    }

    async fn execute_result(self) -> CommandResult<C, SErr> {
        let stream_id = self.command.stream_id();
        let snapshot = self
            .snapshots
            .snapshot_store
            .read_snapshot(ReadSnapshotRequest::new(
                self.snapshots.snapshot_config.clone(),
                stream_id,
            ))
            .await
            .map_err(|source| CommandFailure::ReadSnapshot(source))?;
        let snapshot = snapshot.snapshot;
        let snapshot_position = snapshot.as_ref().map(|snapshot| snapshot.position);
        let state = snapshot
            .map(|snapshot| snapshot.payload)
            .unwrap_or_else(C::initial_state);
        let start_sequence = snapshot_position
            .map(|position| position.get().saturating_add(1))
            .unwrap_or(1);
        let stream_read = self
            .event_store
            .read_stream(ReadStreamRequest::new(stream_id, start_sequence))
            .await
            .map_err(|source| CommandFailure::ReadStream(source))?;
        let current_position = stream_read.current_position;

        if let Some(snapshot_position) = snapshot_position {
            match current_position {
                Some(stream_position) if snapshot_position <= stream_position => {}
                stream_position => {
                    return Err(CommandFailure::SnapshotAheadOfStream {
                        snapshot_position,
                        stream_position,
                    });
                }
            }
        }

        let replayed_event_count = stream_read.events.len() as u64;
        let state = replay_stream_events::<C, EC, SErr>(state, stream_read.events, &self.event_codec)?;
        let (append_outcome, events, state) = self.append_decision::<SErr>(current_position, stream_id, state).await?;
        let events_since_snapshot = replayed_event_count + events.len() as u64;

        let snapshot_decision = self.snapshots.policy.snapshot_decision(SnapshotDecisionContext {
            stream_position: append_outcome.stream_position,
            events_since_snapshot,
            state: &state,
            events: &events,
        });

        if snapshot_decision == SnapshotDecision::Take {
            schedule_snapshot_write(
                &self.snapshots.schedule_snapshot_task,
                self.snapshots.snapshot_store,
                self.snapshots.snapshot_config.clone(),
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

fn box_error<E>(error: E) -> BoxError
where
    E: std::error::Error + Send + Sync + 'static,
{
    Box::new(error)
}

fn resolve_stream_write_precondition<C>(
    write_precondition: Option<StreamWritePrecondition>,
    current_position: Option<StreamPosition>,
) -> StreamWritePrecondition
where
    C: Decider,
{
    C::WRITE_PRECONDITION
        .map(StreamWritePrecondition::from)
        .or(write_precondition)
        .unwrap_or_else(|| StreamWritePrecondition::from_current_position(current_position))
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
    snapshot_config: SnapshotStoreConfig,
    stream_id: &StreamId,
    snapshot: Snapshot<State>,
) where
    S: SnapshotWrite<State, StreamId> + Clone + Send + Sync + 'static,
    S::Error: std::fmt::Display + Send + 'static,
    State: Send + 'static,
    StreamId: AsRef<str> + ToOwned + ?Sized,
    StreamId::Owned: Borrow<StreamId> + Send + 'static,
    Spawn: Fn(BoxTask) + Send + Sync,
{
    let snapshot_store = snapshot_store.clone();
    let stream_id_for_log = stream_id.as_ref().to_string();
    let stream_id = stream_id.to_owned();

    schedule_snapshot_task(Box::pin(async move {
        if let Err(source) = snapshot_store
            .write_snapshot(WriteSnapshotRequest::new(snapshot_config, stream_id.borrow(), snapshot))
            .await
        {
            tracing::warn!(stream_id = %stream_id_for_log, error = %source, "failed to write snapshot");
        }
    }));
}

fn replay_stream_events<C, EC, SErr>(
    mut state: C::State,
    stream_events: Vec<StreamEvent>,
    event_codec: &EC,
) -> ExecutionStep<C, SErr, C::State>
where
    C: Decider,
    EC: EventCodec<C::Event>,
    EC::Error: std::error::Error + Send + Sync + 'static,
{
    for stream_event in stream_events {
        let event = stream_event
            .decode_with(event_codec)
            .map_err(|source| CommandFailure::DecodeEvent(box_error(source)))?;
        state = C::evolve(state, &event).map_err(CommandFailure::Evolve)?;
    }

    Ok(state)
}

fn decision_failure_to_command_failure<C, RuntimeError>(
    failure: DecisionFailure<C::DecideError, C::EvolveError>,
) -> ExecutionFailure<C, RuntimeError>
where
    C: Decider,
{
    match failure {
        DecisionFailure::Decide(error) => CommandFailure::Decide(error),
        DecisionFailure::Evolve(error) => CommandFailure::Evolve(error),
    }
}

fn encode_events<E, C>(codec: &C, events: &Events<E>) -> Result<Events<Event>, EventEncodeError<E, C>>
where
    E: EventType + EventIdentity,
    C: EventCodec<E>,
{
    let first = Event::from_domain_event(codec, events.first())?;
    let rest = events
        .iter()
        .skip(1)
        .map(|event| Event::from_domain_event(codec, event))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Events::from_first(first, rest))
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use chrono::{DateTime, Utc};
    use futures::executor::block_on;
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::{
        CanonicalEventCodec, Decision, EventIdentity, EventType, ReadSnapshotResponse, ReadStreamResponse,
        SnapshotSchema, StreamEvent, WriteSnapshotResponse,
    };

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).expect("test stream position must be non-zero")
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

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestAction {
        Register,
        RegisterThenDisable,
        RegisterThenFail,
        RegisterThenBroken,
        Disable,
        Remove,
        EmitBroken,
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    enum TestState {
        Missing,
        Present { enabled: bool },
    }

    impl SnapshotSchema for TestState {
        const SNAPSHOT_STREAM_PREFIX: &'static str = "test.command.v1.";
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum TestEvent {
        Registered { id: String },
        StateChanged { id: String, enabled: bool },
        Removed { id: String },
        Broken { id: String },
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestDecisionError {
        AlreadyRegistered,
        Missing,
        AlreadyDisabled,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestCommandError {
        BrokenEvent,
        AlreadyRegistered,
        Missing,
        AlreadyDisabled,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestInfraError {
        ReadSnapshot,
        WriteSnapshot,
        ReadStream,
        Append,
        Json,
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
        read_from_sequences: Arc<Mutex<Vec<u64>>>,
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
                read_from_sequences: Arc::new(Mutex::new(Vec::new())),
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
                    .execute(|state, command: &Self| match state {
                        TestState::Present { enabled: true } => Ok(Decision::event(TestEvent::StateChanged {
                            id: command.id.clone(),
                            enabled: false,
                        })),
                        TestState::Present { enabled: false } => Err(TestDecisionError::AlreadyDisabled),
                        TestState::Missing => Err(TestDecisionError::Missing),
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
        type Error = std::convert::Infallible;

        fn event_type(&self) -> Result<&'static str, Self::Error> {
            Ok(match self {
                Self::Registered { .. } => "registered",
                Self::StateChanged { .. } => "state_changed",
                Self::Removed { .. } => "removed",
                Self::Broken { .. } => "broken",
            })
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct TestEventCodec;

    impl EventCodec<TestEvent> for TestEventCodec {
        type Error = serde_json::Error;

        fn encode(&self, value: &TestEvent) -> Result<Vec<u8>, Self::Error> {
            serde_json::to_vec(value)
        }

        fn decode(&self, _event_type: &str, _stream_id: &str, payload: &[u8]) -> Result<TestEvent, Self::Error> {
            serde_json::from_slice(payload)
        }
    }

    impl CanonicalEventCodec for TestEvent {
        type Codec = TestEventCodec;

        fn canonical_codec() -> Self::Codec {
            TestEventCodec
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct WrappedJsonCodec;

    impl EventCodec<TestEvent> for WrappedJsonCodec {
        type Error = TestInfraError;

        fn encode(&self, value: &TestEvent) -> Result<Vec<u8>, Self::Error> {
            serde_json::to_string(value)
                .map(|json| format!("wrapped:{json}"))
                .map(String::into_bytes)
                .map_err(Into::into)
        }

        fn decode(&self, _event_type: &str, _stream_id: &str, payload: &[u8]) -> Result<TestEvent, Self::Error> {
            let value = std::str::from_utf8(payload).map_err(|_| TestInfraError::Json)?;
            let json = value.strip_prefix("wrapped:").ok_or(TestInfraError::Json)?;
            serde_json::from_str(json).map_err(Into::into)
        }
    }

    fn test_snapshot_config() -> SnapshotStoreConfig {
        TestState::snapshot_store_config()
    }

    fn test_snapshots<P>(runtime: &FakeRuntime, policy: P) -> Snapshots<'_, FakeRuntime, P, fn(BoxTask)> {
        Snapshots::new(runtime, test_snapshot_config(), policy)
            .schedule_snapshot_tasks_with(run_task_immediately as fn(BoxTask))
    }

    impl StreamRead<str> for FakeRuntime {
        type Error = TestInfraError;

        async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
            if self.fail_read_stream {
                return Err(TestInfraError::ReadStream);
            }
            self.read_from_sequences.lock().unwrap().push(request.from_sequence);
            Ok(ReadStreamResponse {
                current_position: self.current_position,
                events: self
                    .stream_events
                    .iter()
                    .filter(|event| event.stream_position.get() >= request.from_sequence)
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
            self.appended_events.lock().unwrap().extend(request.events.into_vec());
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
            Ok(ReadSnapshotResponse::new(self.snapshot.clone()))
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
            Ok(WriteSnapshotResponse::new())
        }
    }

    fn stream_event(sequence: u64, event: TestEvent) -> StreamEvent {
        let stream_id = match &event {
            TestEvent::Registered { id }
            | TestEvent::StateChanged { id, .. }
            | TestEvent::Removed { id }
            | TestEvent::Broken { id } => id.clone(),
        };
        Event::from_domain_event(&TestEventCodec, &event).unwrap().record(
            stream_id,
            position(sequence),
            DateTime::<Utc>::from_timestamp(1_700_000_000 + sequence as i64, 0).unwrap(),
        )
    }

    #[test]
    fn executes_from_initial_state_without_snapshot_or_history() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let result = block_on(CommandExecution::new(&runtime, &command).execute_result()).unwrap();

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
        assert_eq!(runtime.read_from_sequences.lock().unwrap().as_slice(), &[1]);
    }

    #[test]
    fn executes_act_decisions_with_evolved_step_state() {
        let runtime = FakeRuntime {
            stream_position: position(2),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::RegisterThenDisable);

        let result = block_on(CommandExecution::new(&runtime, &command).execute_result()).unwrap();

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
                .execute_result(),
        )
        .unwrap();

        assert_eq!(runtime.read_from_sequences.lock().unwrap().as_slice(), &[2]);
        assert_eq!(
            runtime.stream_write_preconditions.lock().unwrap().as_slice(),
            &[StreamWritePrecondition::At(position(2))]
        );
        assert_eq!(result.state, TestState::Missing);
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
                .execute_result(),
        )
        .unwrap_err();

        let CommandFailure::SnapshotAheadOfStream {
            snapshot_position,
            stream_position,
        } = error
        else {
            panic!("expected snapshot ahead of stream");
        };
        assert_eq!(snapshot_position, position(3));
        assert_eq!(stream_position, Some(position(2)));
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
                .execute_result(),
        )
        .unwrap_err();

        let CommandFailure::SnapshotAheadOfStream {
            snapshot_position,
            stream_position,
        } = error
        else {
            panic!("expected snapshot ahead of stream");
        };
        assert_eq!(snapshot_position, position(1));
        assert_eq!(stream_position, None);
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

        let error = block_on(CommandExecution::new(&runtime, &command).execute_result()).unwrap_err();

        assert!(matches!(error, CommandFailure::Evolve(TestCommandError::BrokenEvent)));
    }

    #[test]
    fn does_not_append_events_that_cannot_evolve() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::EmitBroken);

        let error = block_on(CommandExecution::new(&runtime, &command).execute_result()).unwrap_err();

        assert!(matches!(error, CommandFailure::Evolve(TestCommandError::BrokenEvent)));
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

        let error = block_on(CommandExecution::new(&runtime, &command).execute_result()).unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Decide(TestDecisionError::AlreadyDisabled)
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

        let error = block_on(CommandExecution::new(&runtime, &command).execute_result()).unwrap_err();

        assert!(matches!(error, CommandFailure::Evolve(TestCommandError::BrokenEvent)));
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
                .execute_result(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Decide(TestDecisionError::AlreadyRegistered)
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
                .execute_result(),
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
    fn supports_custom_event_codecs() {
        let runtime = FakeRuntime {
            current_position: Some(position(1)),
            stream_events: vec![
                Event::from_domain_event(
                    &WrappedJsonCodec,
                    &TestEvent::Registered {
                        id: "alpha".to_string(),
                    },
                )
                .unwrap()
                .record(
                    "alpha",
                    position(1),
                    DateTime::<Utc>::from_timestamp(1_700_000_001, 0).unwrap(),
                ),
            ],
            stream_position: position(2),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Disable);

        let result = block_on(
            CommandExecution::new(&runtime, &command)
                .with_codec(WrappedJsonCodec)
                .execute_result(),
        )
        .unwrap();
        let appended_events = runtime.appended_events.lock().unwrap();

        assert_eq!(result.state, TestState::Present { enabled: false });
        assert_eq!(appended_events.len(), 1);
        assert!(appended_events[0].content.starts_with(b"wrapped:"));
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

        let _ = block_on(CommandExecution::new(&runtime, &command).execute_result()).unwrap();

        assert_eq!(
            runtime.stream_write_preconditions.lock().unwrap().as_slice(),
            &[StreamWritePrecondition::At(position(7))]
        );
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
                .execute_result(),
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
                .execute_result(),
        )
        .unwrap();

        assert_eq!(
            runtime.stream_write_preconditions.lock().unwrap().as_slice(),
            &[StreamWritePrecondition::NoStream]
        );
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
                .with_snapshot(test_snapshots(&runtime, FrequencySnapshot::new(NonZeroU64::MIN)))
                .execute_result(),
        )
        .unwrap();

        assert_eq!(result.state, TestState::Present { enabled: true });
        assert_eq!(
            runtime.written_snapshots.lock().unwrap().as_slice(),
            &[Snapshot::new(position(1), TestState::Present { enabled: true })]
        );
    }

    #[test]
    fn frequency_snapshot_writes_after_enough_events_since_snapshot() {
        const EVERY_TWO_EVENTS: NonZeroU64 = NonZeroU64::new(2).expect("snapshot cadence must be non-zero");
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
                .execute_result(),
        )
        .unwrap();

        assert_eq!(
            runtime.written_snapshots.lock().unwrap().as_slice(),
            &[Snapshot::new(position(3), TestState::Present { enabled: false })]
        );
    }

    #[test]
    fn frequency_snapshot_skips_before_enough_events_since_snapshot() {
        const EVERY_TWO_EVENTS: NonZeroU64 = NonZeroU64::new(2).expect("snapshot cadence must be non-zero");
        let runtime = FakeRuntime {
            stream_position: position(2),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(test_snapshots(&runtime, FrequencySnapshot::new(EVERY_TWO_EVENTS)))
                .execute_result(),
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
                .execute_result(),
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
                .execute_result(),
        )
        .unwrap();

        assert_eq!(result.stream_position, position(1));
        assert_eq!(result.state, TestState::Present { enabled: true });
        assert!(runtime.written_snapshots.lock().unwrap().is_empty());
    }

    #[test]
    fn resolves_stream_id_once() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(CommandExecution::new(&runtime, &command).execute_result()).unwrap();

        assert_eq!(command.stream_id_calls(), 1);
        assert!(runtime.loaded_stream_ids.lock().unwrap().is_empty());
    }
}
