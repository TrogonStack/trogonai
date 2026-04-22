use crate::{
    CanonicalEventCodec, Decide, Decision, EventCodec, EventData, EventType, NonEmpty, RecordedEvent, Snapshot,
    SnapshotSchema, SnapshotStoreConfig, StateMachine, StreamEvent,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    Any,
    StreamExists,
    NoStream,
    StreamRevision(u64),
}

impl StreamState {
    pub const fn from_current_version(current_version: Option<u64>) -> Self {
        match current_version {
            Some(version) => Self::StreamRevision(version),
            None => Self::NoStream,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccPolicy {
    ExactCurrentVersion,
    Explicit(StreamState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePrecondition {
    Prefer(StreamState),
    Require(StreamState),
}

pub const fn resolve_stream_state(
    occ: Option<OccPolicy>,
    current_version: Option<u64>,
    write_precondition: Option<WritePrecondition>,
) -> StreamState {
    if let Some(WritePrecondition::Require(stream_state)) = write_precondition {
        return stream_state;
    }

    match occ {
        Some(OccPolicy::ExactCurrentVersion) => StreamState::from_current_version(current_version),
        Some(OccPolicy::Explicit(stream_state)) => stream_state,
        None => match write_precondition {
            Some(WritePrecondition::Prefer(stream_state)) => stream_state,
            Some(WritePrecondition::Require(stream_state)) => stream_state,
            None => StreamState::from_current_version(current_version),
        },
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StreamReadResult {
    pub current_version: Option<u64>,
    pub events: Vec<RecordedEvent>,
}

pub trait StreamRead<StreamId: ?Sized>: Send + Sync {
    type Error;

    fn read_stream_from(
        &self,
        stream_id: &StreamId,
        from_sequence: u64,
    ) -> impl std::future::Future<Output = Result<StreamReadResult, Self::Error>> + Send;
}

pub trait StreamAppend<StreamId: ?Sized>: Send + Sync {
    type Error;

    fn append_events(
        &self,
        stream_id: &StreamId,
        stream_state: StreamState,
        events: NonEmpty<EventData>,
    ) -> impl std::future::Future<Output = Result<AppendOutcome, Self::Error>> + Send;
}

pub trait SnapshotRead<SnapshotPayload, StreamId: ?Sized>: Send + Sync {
    type Error;

    fn load_snapshot(
        &self,
        config: SnapshotStoreConfig,
        stream_id: &StreamId,
    ) -> impl std::future::Future<Output = Result<Option<Snapshot<SnapshotPayload>>, Self::Error>> + Send;
}

pub trait SnapshotWrite<SnapshotPayload, StreamId: ?Sized>: Send + Sync {
    type Error;

    fn save_snapshot(
        &self,
        config: SnapshotStoreConfig,
        stream_id: &StreamId,
        snapshot: Snapshot<SnapshotPayload>,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;
}

use std::num::NonZeroU64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendOutcome {
    pub next_expected_version: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotDecision {
    Skip,
    Take,
}

pub trait SnapshotPolicy<State, Event> {
    fn snapshot_decision(
        &self,
        next_expected_version: u64,
        state: &State,
        events: &NonEmpty<Event>,
    ) -> SnapshotDecision;
}

pub trait CommandSnapshotPolicy: Decide
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
    fn snapshot_decision(
        &self,
        _next_expected_version: u64,
        _state: &State,
        _events: &NonEmpty<Event>,
    ) -> SnapshotDecision {
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
    fn snapshot_decision(
        &self,
        next_expected_version: u64,
        _state: &State,
        _events: &NonEmpty<Event>,
    ) -> SnapshotDecision {
        if next_expected_version.is_multiple_of(self.frequency.get()) {
            SnapshotDecision::Take
        } else {
            SnapshotDecision::Skip
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionResult<State, Event> {
    pub next_expected_version: u64,
    pub events: NonEmpty<Event>,
    pub state: State,
}

pub trait StateMachineCommand: Decide {
    type EvolveError;
}

impl<C> StateMachineCommand for C
where
    C: Decide,
    C::State: StateMachine<C::Event>,
{
    type EvolveError = <C::State as StateMachine<C::Event>>::EvolveError;
}

pub type CommandResult<C, InfraError> = Result<
    ExecutionResult<<C as Decide>::State, <C as Decide>::Event>,
    CommandFailure<<C as Decide>::DecideError, <C as StateMachineCommand>::EvolveError, CommandInfraError<InfraError>>,
>;

#[derive(Debug)]
pub enum CommandFailure<DecideError, EvolveError, InfraError> {
    Decide(DecideError),
    Evolve(EvolveError),
    Infra(InfraError),
}

#[derive(Debug)]
pub enum CommandInfraError<RuntimeError> {
    LoadSnapshot(RuntimeError),
    SaveSnapshot(RuntimeError),
    ReadStream(RuntimeError),
    Append(RuntimeError),
    EncodeEvent(RuntimeError),
    DecodeEvent(RuntimeError),
    SnapshotAheadOfStream {
        snapshot_version: u64,
        stream_version: Option<u64>,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WithoutSnapshots;

pub struct Snapshots<'a, S, P> {
    snapshot_store: &'a S,
    snapshot_config: SnapshotStoreConfig,
    policy: P,
}

impl<'a, S, P> Snapshots<'a, S, P> {
    pub fn new(snapshot_store: &'a S, snapshot_config: SnapshotStoreConfig, policy: P) -> Self {
        Self {
            snapshot_store,
            snapshot_config,
            policy,
        }
    }
}

pub trait IntoSnapshots<'a, C>: Sized
where
    C: Decide,
{
    type Store;
    type Policy;

    fn into_snapshots(self) -> Snapshots<'a, Self::Store, Self::Policy>;
}

impl<'a, C, S, P> IntoSnapshots<'a, C> for Snapshots<'a, S, P>
where
    C: Decide,
{
    type Store = S;
    type Policy = P;

    fn into_snapshots(self) -> Snapshots<'a, Self::Store, Self::Policy> {
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

    fn into_snapshots(self) -> Snapshots<'a, Self::Store, Self::Policy> {
        C::snapshots(self)
    }
}

pub struct CommandExecution<'a, E, C, S = WithoutSnapshots> {
    event_store: &'a E,
    command: &'a C,
    occ: Option<OccPolicy>,
    snapshots: S,
}

impl<'a, E, C> CommandExecution<'a, E, C, WithoutSnapshots>
where
    C: Decide,
{
    pub const fn new(event_store: &'a E, command: &'a C) -> Self {
        Self {
            event_store,
            command,
            occ: None,
            snapshots: WithoutSnapshots,
        }
    }

    pub fn with_snapshot<I>(self, snapshots: I) -> CommandExecution<'a, E, C, Snapshots<'a, I::Store, I::Policy>>
    where
        I: IntoSnapshots<'a, C>,
    {
        CommandExecution {
            event_store: self.event_store,
            command: self.command,
            occ: self.occ,
            snapshots: snapshots.into_snapshots(),
        }
    }
}

impl<'a, E, C, S> CommandExecution<'a, E, C, S> {
    pub fn with_occ<O>(mut self, occ: O) -> Self
    where
        O: Into<Option<OccPolicy>>,
    {
        self.occ = occ.into();
        self
    }
}

impl<'a, E, C, S> CommandExecution<'a, E, C, S> {
    pub fn with_codec<EC>(self, event_codec: EC) -> CommandExecutionWithCodec<'a, E, C, S, EC> {
        CommandExecutionWithCodec {
            event_store: self.event_store,
            command: self.command,
            occ: self.occ,
            snapshots: self.snapshots,
            event_codec,
        }
    }
}

pub struct CommandExecutionWithCodec<'a, E, C, S, EC> {
    event_store: &'a E,
    command: &'a C,
    occ: Option<OccPolicy>,
    snapshots: S,
    event_codec: EC,
}

impl<'a, E, C, S, EC> CommandExecutionWithCodec<'a, E, C, S, EC>
where
    C: Decide,
{
    pub fn with_occ<O>(mut self, occ: O) -> Self
    where
        O: Into<Option<OccPolicy>>,
    {
        self.occ = occ.into();
        self
    }

    pub fn with_snapshot<I>(
        self,
        snapshots: I,
    ) -> CommandExecutionWithCodec<'a, E, C, Snapshots<'a, I::Store, I::Policy>, EC>
    where
        I: IntoSnapshots<'a, C>,
    {
        CommandExecutionWithCodec {
            event_store: self.event_store,
            command: self.command,
            occ: self.occ,
            snapshots: snapshots.into_snapshots(),
            event_codec: self.event_codec,
        }
    }
}

impl<E, C, SErr> CommandExecution<'_, E, C, WithoutSnapshots>
where
    C: Decide,
    C::State: StateMachine<C::Event>,
    C::Event: EventType + StreamEvent + Clone + CanonicalEventCodec,
    E: StreamRead<C::StreamId, Error = SErr> + StreamAppend<C::StreamId, Error = SErr>,
    <C::Event as CanonicalEventCodec>::Codec: EventCodec<C::Event>,
    <<C::Event as CanonicalEventCodec>::Codec as EventCodec<C::Event>>::Error: Into<SErr>,
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
    C: Decide,
    C::State: StateMachine<C::Event>,
    C::Event: EventType + StreamEvent + Clone,
    E: StreamRead<C::StreamId, Error = SErr> + StreamAppend<C::StreamId, Error = SErr>,
    EC: EventCodec<C::Event>,
    EC::Error: Into<SErr>,
{
    pub async fn execute(self) -> CommandResult<C, SErr> {
        self.execute_result().await
    }

    async fn execute_result(self) -> CommandResult<C, SErr> {
        let stream_id = self.command.stream_id();
        let stream_read = self
            .event_store
            .read_stream_from(stream_id, 1)
            .await
            .map_err(CommandInfraError::ReadStream)
            .map_err(CommandFailure::Infra)?;
        let current_version = stream_read.current_version;
        let mut state = C::State::initial_state();

        for recorded_event in stream_read.events {
            let event = recorded_event
                .decode_data_with(&self.event_codec)
                .map_err(Into::into)
                .map_err(CommandInfraError::DecodeEvent)
                .map_err(CommandFailure::Infra)?;
            state = state.evolve(event).map_err(CommandFailure::Evolve)?;
        }

        let Decision::Event(events) = C::decide(&state, self.command).map_err(CommandFailure::Decide)?;
        let encoded_events = encode_events(&self.event_codec, &events)
            .map_err(Into::into)
            .map_err(CommandInfraError::EncodeEvent)
            .map_err(CommandFailure::Infra)?;
        let stream_state = resolve_stream_state(self.occ, current_version, self.command.write_precondition());
        let append_outcome = self
            .event_store
            .append_events(stream_id, stream_state, encoded_events)
            .await
            .map_err(CommandInfraError::Append)
            .map_err(CommandFailure::Infra)?;

        for event in events.iter().cloned() {
            state = state.evolve(event).map_err(CommandFailure::Evolve)?;
        }

        Ok(ExecutionResult {
            next_expected_version: append_outcome.next_expected_version,
            events,
            state,
        })
    }
}

impl<E, S, C, P, SErr> CommandExecution<'_, E, C, Snapshots<'_, S, P>>
where
    C: Decide,
    C::State: Clone,
    C::State: StateMachine<C::Event>,
    C::Event: EventType + StreamEvent + Clone + CanonicalEventCodec,
    E: StreamRead<C::StreamId, Error = SErr> + StreamAppend<C::StreamId, Error = SErr>,
    S: SnapshotRead<C::State, C::StreamId, Error = SErr> + SnapshotWrite<C::State, C::StreamId, Error = SErr>,
    P: SnapshotPolicy<C::State, C::Event>,
    <C::Event as CanonicalEventCodec>::Codec: EventCodec<C::Event>,
    <<C::Event as CanonicalEventCodec>::Codec as EventCodec<C::Event>>::Error: Into<SErr>,
{
    pub async fn execute(self) -> CommandResult<C, SErr> {
        self.execute_result().await
    }

    async fn execute_result(self) -> CommandResult<C, SErr> {
        self.with_codec(C::Event::canonical_codec()).execute_result().await
    }
}

impl<E, S, C, P, SErr, EC> CommandExecutionWithCodec<'_, E, C, Snapshots<'_, S, P>, EC>
where
    C: Decide,
    C::State: Clone,
    C::State: StateMachine<C::Event>,
    C::Event: EventType + StreamEvent + Clone,
    E: StreamRead<C::StreamId, Error = SErr> + StreamAppend<C::StreamId, Error = SErr>,
    S: SnapshotRead<C::State, C::StreamId, Error = SErr> + SnapshotWrite<C::State, C::StreamId, Error = SErr>,
    P: SnapshotPolicy<C::State, C::Event>,
    EC: EventCodec<C::Event>,
    EC::Error: Into<SErr>,
{
    pub async fn execute(self) -> CommandResult<C, SErr> {
        self.execute_result().await
    }

    async fn execute_result(self) -> CommandResult<C, SErr> {
        let stream_id = self.command.stream_id();
        let snapshot = self
            .snapshots
            .snapshot_store
            .load_snapshot(self.snapshots.snapshot_config.clone(), stream_id)
            .await
            .map_err(CommandInfraError::LoadSnapshot)
            .map_err(CommandFailure::Infra)?;
        let snapshot_version = snapshot.as_ref().map(|snapshot| snapshot.version);
        let mut state = snapshot
            .map(|snapshot| snapshot.payload)
            .unwrap_or_else(C::State::initial_state);
        let start_sequence = snapshot_version.map(|version| version.saturating_add(1)).unwrap_or(1);
        let stream_read = self
            .event_store
            .read_stream_from(stream_id, start_sequence)
            .await
            .map_err(CommandInfraError::ReadStream)
            .map_err(CommandFailure::Infra)?;
        let current_version = stream_read.current_version;

        if let Some(snapshot_version) = snapshot_version {
            match current_version {
                Some(stream_version) if snapshot_version <= stream_version => {}
                stream_version => {
                    return Err(CommandFailure::Infra(CommandInfraError::SnapshotAheadOfStream {
                        snapshot_version,
                        stream_version,
                    }));
                }
            }
        }

        for recorded_event in stream_read.events {
            let event = recorded_event
                .decode_data_with(&self.event_codec)
                .map_err(Into::into)
                .map_err(CommandInfraError::DecodeEvent)
                .map_err(CommandFailure::Infra)?;
            state = state.evolve(event).map_err(CommandFailure::Evolve)?;
        }

        let Decision::Event(events) = C::decide(&state, self.command).map_err(CommandFailure::Decide)?;
        let encoded_events = encode_events(&self.event_codec, &events)
            .map_err(Into::into)
            .map_err(CommandInfraError::EncodeEvent)
            .map_err(CommandFailure::Infra)?;
        let stream_state = resolve_stream_state(self.occ, current_version, self.command.write_precondition());
        let append_outcome = self
            .event_store
            .append_events(stream_id, stream_state, encoded_events)
            .await
            .map_err(CommandInfraError::Append)
            .map_err(CommandFailure::Infra)?;

        for event in events.iter().cloned() {
            state = state.evolve(event).map_err(CommandFailure::Evolve)?;
        }

        if matches!(
            self.snapshots
                .policy
                .snapshot_decision(append_outcome.next_expected_version, &state, &events,),
            SnapshotDecision::Take
        ) {
            self.snapshots
                .snapshot_store
                .save_snapshot(
                    self.snapshots.snapshot_config.clone(),
                    stream_id,
                    Snapshot::new(append_outcome.next_expected_version, state.clone()),
                )
                .await
                .map_err(CommandInfraError::SaveSnapshot)
                .map_err(CommandFailure::Infra)?;
        }

        Ok(ExecutionResult {
            next_expected_version: append_outcome.next_expected_version,
            events,
            state,
        })
    }
}

fn encode_events<E, C>(codec: &C, events: &NonEmpty<E>) -> Result<NonEmpty<EventData>, C::Error>
where
    E: EventType + StreamEvent + Clone,
    C: EventCodec<E>,
{
    events.clone().try_map(|event| EventData::new_with_codec(codec, event))
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
    use crate::{CanonicalEventCodec, EventType, SnapshotSchema, StreamCommand, StreamEvent};

    #[derive(Debug, Clone)]
    struct TestCommand {
        id: String,
        action: TestAction,
        write_precondition: Option<WritePrecondition>,
        stream_id_calls: Arc<AtomicUsize>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestAction {
        Register,
        Disable,
        Remove,
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
        LoadSnapshot,
        SaveSnapshot,
        ReadStream,
        Append,
        Json,
    }

    impl From<serde_json::Error> for TestInfraError {
        fn from(_value: serde_json::Error) -> Self {
            Self::Json
        }
    }

    impl From<TestDecisionError> for TestCommandError {
        fn from(value: TestDecisionError) -> Self {
            match value {
                TestDecisionError::AlreadyRegistered => Self::AlreadyRegistered,
                TestDecisionError::Missing => Self::Missing,
                TestDecisionError::AlreadyDisabled => Self::AlreadyDisabled,
            }
        }
    }

    #[derive(Debug, Default, Clone)]
    struct FakeRuntime {
        snapshot: Option<Snapshot<TestState>>,
        current_version: Option<u64>,
        recorded_events: Vec<RecordedEvent>,
        next_expected_version: u64,
        fail_load_snapshot: bool,
        fail_save_snapshot: bool,
        fail_read_stream: bool,
        fail_append: bool,
        loaded_stream_ids: Arc<Mutex<Vec<String>>>,
        read_from_sequences: Arc<Mutex<Vec<u64>>>,
        stream_states: Arc<Mutex<Vec<StreamState>>>,
        appended_events: Arc<Mutex<Vec<EventData>>>,
        saved_snapshots: Arc<Mutex<Vec<Snapshot<TestState>>>>,
    }

    impl TestCommand {
        fn new(id: &str, action: TestAction) -> Self {
            Self {
                id: id.to_string(),
                action,
                write_precondition: None,
                stream_id_calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn with_preferred_write_precondition(mut self, stream_state: StreamState) -> Self {
            self.write_precondition = Some(WritePrecondition::Prefer(stream_state));
            self
        }

        fn with_required_write_precondition(mut self, stream_state: StreamState) -> Self {
            self.write_precondition = Some(WritePrecondition::Require(stream_state));
            self
        }

        fn stream_id_calls(&self) -> usize {
            self.stream_id_calls.load(Ordering::SeqCst)
        }
    }

    impl StreamCommand for TestCommand {
        type StreamId = str;

        fn stream_id(&self) -> &Self::StreamId {
            self.stream_id_calls.fetch_add(1, Ordering::SeqCst);
            &self.id
        }

        fn write_precondition(&self) -> Option<WritePrecondition> {
            self.write_precondition
        }
    }

    impl StateMachine<TestEvent> for TestState {
        type EvolveError = TestCommandError;

        fn initial_state() -> Self {
            TestState::Missing
        }

        fn evolve(self, event: TestEvent) -> Result<Self, Self::EvolveError> {
            match event {
                TestEvent::Registered { .. } => Ok(TestState::Present { enabled: true }),
                TestEvent::StateChanged { enabled, .. } => Ok(TestState::Present { enabled }),
                TestEvent::Removed { .. } => Ok(TestState::Missing),
                TestEvent::Broken { .. } => Err(TestCommandError::BrokenEvent),
            }
        }
    }

    impl Decide for TestCommand {
        type State = TestState;
        type Event = TestEvent;
        type DecideError = TestDecisionError;

        fn decide(state: &TestState, command: &Self) -> Result<Decision<TestEvent>, Self::DecideError> {
            match (state, command.action) {
                (TestState::Missing, TestAction::Register) => {
                    Ok(Decision::event(TestEvent::Registered { id: command.id.clone() }))
                }
                (TestState::Present { .. }, TestAction::Register) => Err(TestDecisionError::AlreadyRegistered),
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

    impl StreamEvent for TestEvent {
        fn stream_id(&self) -> &str {
            match self {
                Self::Registered { id }
                | Self::StateChanged { id, .. }
                | Self::Removed { id }
                | Self::Broken { id } => id,
            }
        }
    }

    impl EventType for TestEvent {
        fn event_type(&self) -> &'static str {
            match self {
                Self::Registered { .. } => "registered",
                Self::StateChanged { .. } => "state_changed",
                Self::Removed { .. } => "removed",
                Self::Broken { .. } => "broken",
            }
        }
    }

    impl CanonicalEventCodec for TestEvent {
        type Codec = crate::JsonEventCodec;

        fn canonical_codec() -> Self::Codec {
            crate::JsonEventCodec
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct WrappedJsonCodec;

    impl EventCodec<TestEvent> for WrappedJsonCodec {
        type Error = TestInfraError;

        fn encode(&self, value: &TestEvent) -> Result<String, Self::Error> {
            serde_json::to_string(value)
                .map(|json| format!("wrapped:{json}"))
                .map_err(Into::into)
        }

        fn decode(&self, value: &str) -> Result<TestEvent, Self::Error> {
            let json = value.strip_prefix("wrapped:").ok_or(TestInfraError::Json)?;
            serde_json::from_str(json).map_err(Into::into)
        }
    }

    fn test_snapshot_config() -> SnapshotStoreConfig {
        TestState::snapshot_store_config()
    }

    impl StreamRead<str> for FakeRuntime {
        type Error = TestInfraError;

        async fn read_stream_from(
            &self,
            _stream_id: &str,
            from_sequence: u64,
        ) -> Result<StreamReadResult, Self::Error> {
            if self.fail_read_stream {
                return Err(TestInfraError::ReadStream);
            }
            self.read_from_sequences.lock().unwrap().push(from_sequence);
            Ok(StreamReadResult {
                current_version: self.current_version,
                events: self
                    .recorded_events
                    .iter()
                    .filter(|event| event.log_position.unwrap_or(0) >= from_sequence)
                    .cloned()
                    .collect(),
            })
        }
    }

    impl StreamAppend<str> for FakeRuntime {
        type Error = TestInfraError;

        async fn append_events(
            &self,
            _stream_id: &str,
            stream_state: StreamState,
            events: NonEmpty<EventData>,
        ) -> Result<AppendOutcome, Self::Error> {
            if self.fail_append {
                return Err(TestInfraError::Append);
            }
            self.stream_states.lock().unwrap().push(stream_state);
            self.appended_events.lock().unwrap().extend(events.into_vec());
            Ok(AppendOutcome {
                next_expected_version: self.next_expected_version,
            })
        }
    }

    impl SnapshotRead<TestState, str> for FakeRuntime {
        type Error = TestInfraError;

        async fn load_snapshot(
            &self,
            _config: SnapshotStoreConfig,
            stream_id: &str,
        ) -> Result<Option<Snapshot<TestState>>, Self::Error> {
            if self.fail_load_snapshot {
                return Err(TestInfraError::LoadSnapshot);
            }
            self.loaded_stream_ids.lock().unwrap().push(stream_id.to_string());
            Ok(self.snapshot.clone())
        }
    }

    impl SnapshotWrite<TestState, str> for FakeRuntime {
        type Error = TestInfraError;

        async fn save_snapshot(
            &self,
            _config: SnapshotStoreConfig,
            _stream_id: &str,
            snapshot: Snapshot<TestState>,
        ) -> Result<(), Self::Error> {
            if self.fail_save_snapshot {
                return Err(TestInfraError::SaveSnapshot);
            }
            self.saved_snapshots.lock().unwrap().push(snapshot);
            Ok(())
        }
    }

    fn recorded_event(sequence: u64, event: TestEvent) -> RecordedEvent {
        EventData::new(event).unwrap().record(
            "stream-alpha",
            Some(sequence),
            Some(sequence),
            DateTime::<Utc>::from_timestamp(1_700_000_000 + sequence as i64, 0).unwrap(),
        )
    }

    #[test]
    fn executes_from_initial_state_without_snapshot_or_history() {
        let runtime = FakeRuntime {
            next_expected_version: 1,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let result = block_on(CommandExecution::new(&runtime, &command).execute_result()).unwrap();

        assert_eq!(result.next_expected_version, 1);
        assert_eq!(result.state, TestState::Present { enabled: true });
        assert_eq!(
            result.events,
            NonEmpty::one(TestEvent::Registered {
                id: "alpha".to_string(),
            })
        );
        assert_eq!(
            runtime.stream_states.lock().unwrap().as_slice(),
            &[StreamState::NoStream]
        );
        assert_eq!(runtime.read_from_sequences.lock().unwrap().as_slice(), &[1]);
    }

    #[test]
    fn restores_from_snapshot_and_reads_only_delta_after_snapshot_version() {
        let runtime = FakeRuntime {
            snapshot: Some(Snapshot::new(1, TestState::Present { enabled: true })),
            current_version: Some(2),
            recorded_events: vec![
                recorded_event(
                    1,
                    TestEvent::Registered {
                        id: "alpha".to_string(),
                    },
                ),
                recorded_event(
                    2,
                    TestEvent::StateChanged {
                        id: "alpha".to_string(),
                        enabled: false,
                    },
                ),
            ],
            next_expected_version: 3,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Remove);

        let result = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(Snapshots::new(&runtime, test_snapshot_config(), NoSnapshot))
                .execute_result(),
        )
        .unwrap();

        assert_eq!(runtime.read_from_sequences.lock().unwrap().as_slice(), &[2]);
        assert_eq!(
            runtime.stream_states.lock().unwrap().as_slice(),
            &[StreamState::StreamRevision(2)]
        );
        assert_eq!(result.state, TestState::Missing);
    }

    #[test]
    fn errors_when_snapshot_is_ahead_of_stream() {
        let runtime = FakeRuntime {
            snapshot: Some(Snapshot::new(3, TestState::Present { enabled: true })),
            current_version: Some(2),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Remove);

        let error = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(Snapshots::new(&runtime, test_snapshot_config(), NoSnapshot))
                .execute_result(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Infra(CommandInfraError::SnapshotAheadOfStream {
                snapshot_version: 3,
                stream_version: Some(2),
            })
        ));
    }

    #[test]
    fn errors_when_snapshot_exists_without_stream_history() {
        let runtime = FakeRuntime {
            snapshot: Some(Snapshot::new(1, TestState::Present { enabled: true })),
            current_version: None,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Remove);

        let error = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(Snapshots::new(&runtime, test_snapshot_config(), NoSnapshot))
                .execute_result(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Infra(CommandInfraError::SnapshotAheadOfStream {
                snapshot_version: 1,
                stream_version: None,
            })
        ));
    }

    #[test]
    fn propagates_replay_evolve_failures() {
        let runtime = FakeRuntime {
            current_version: Some(1),
            recorded_events: vec![recorded_event(
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
    fn propagates_decision_failures() {
        let runtime = FakeRuntime {
            snapshot: Some(Snapshot::new(1, TestState::Present { enabled: true })),
            current_version: Some(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let error = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(Snapshots::new(&runtime, test_snapshot_config(), NoSnapshot))
                .execute_result(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Decide(TestDecisionError::AlreadyRegistered)
        ));
    }

    #[test]
    fn encodes_emitted_events_and_returns_next_expected_version() {
        let runtime = FakeRuntime {
            snapshot: Some(Snapshot::new(1, TestState::Present { enabled: true })),
            current_version: Some(1),
            next_expected_version: 2,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Disable);

        let result = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(Snapshots::new(&runtime, test_snapshot_config(), NoSnapshot))
                .execute_result(),
        )
        .unwrap();
        let appended_events = runtime.appended_events.lock().unwrap();

        assert_eq!(result.next_expected_version, 2);
        assert_eq!(
            runtime.stream_states.lock().unwrap().as_slice(),
            &[StreamState::StreamRevision(1)]
        );
        assert_eq!(appended_events.len(), 1);
        assert_eq!(appended_events[0].event_type, "state_changed");
        assert_eq!(appended_events[0].stream_id, "alpha");
        assert_eq!(result.state, TestState::Present { enabled: false });
    }

    #[test]
    fn supports_custom_event_codecs() {
        let runtime = FakeRuntime {
            current_version: Some(1),
            recorded_events: vec![
                EventData::new_with_codec(
                    &WrappedJsonCodec,
                    TestEvent::Registered {
                        id: "alpha".to_string(),
                    },
                )
                .unwrap()
                .record(
                    "stream-alpha",
                    Some(1),
                    Some(1),
                    DateTime::<Utc>::from_timestamp(1_700_000_001, 0).unwrap(),
                ),
            ],
            next_expected_version: 2,
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
        assert!(appended_events[0].data.starts_with("wrapped:"));
    }

    #[test]
    fn uses_command_default_expected_state_when_using_command_rule() {
        let runtime = FakeRuntime {
            next_expected_version: 1,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register)
            .with_preferred_write_precondition(StreamState::StreamExists);

        let _ = block_on(CommandExecution::new(&runtime, &command).execute_result()).unwrap();

        assert_eq!(
            runtime.stream_states.lock().unwrap().as_slice(),
            &[StreamState::StreamExists]
        );
    }

    #[test]
    fn explicit_occ_overrides_default_command_rule() {
        let runtime = FakeRuntime {
            next_expected_version: 1,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register)
            .with_preferred_write_precondition(StreamState::StreamExists);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .with_occ(OccPolicy::Explicit(StreamState::Any))
                .execute_result(),
        )
        .unwrap();

        assert_eq!(runtime.stream_states.lock().unwrap().as_slice(), &[StreamState::Any]);
    }

    #[test]
    fn exact_current_version_overrides_default_command_rule() {
        let runtime = FakeRuntime {
            current_version: Some(7),
            recorded_events: vec![recorded_event(
                7,
                TestEvent::Registered {
                    id: "alpha".to_string(),
                },
            )],
            next_expected_version: 8,
            ..Default::default()
        };
        let command =
            TestCommand::new("alpha", TestAction::Remove).with_preferred_write_precondition(StreamState::StreamExists);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .with_occ(OccPolicy::ExactCurrentVersion)
                .execute_result(),
        )
        .unwrap();

        assert_eq!(
            runtime.stream_states.lock().unwrap().as_slice(),
            &[StreamState::StreamRevision(7)]
        );
    }

    #[test]
    fn required_command_rule_ignores_explicit_occ() {
        let runtime = FakeRuntime {
            next_expected_version: 1,
            ..Default::default()
        };
        let command =
            TestCommand::new("alpha", TestAction::Register).with_required_write_precondition(StreamState::NoStream);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .with_occ(OccPolicy::Explicit(StreamState::Any))
                .execute_result(),
        )
        .unwrap();

        assert_eq!(
            runtime.stream_states.lock().unwrap().as_slice(),
            &[StreamState::NoStream]
        );
    }

    #[test]
    fn saves_snapshot_when_policy_requests_it() {
        let runtime = FakeRuntime {
            next_expected_version: 1,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let result = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(Snapshots::new(
                    &runtime,
                    test_snapshot_config(),
                    FrequencySnapshot::new(NonZeroU64::MIN),
                ))
                .execute_result(),
        )
        .unwrap();

        assert_eq!(result.state, TestState::Present { enabled: true });
        assert_eq!(
            runtime.saved_snapshots.lock().unwrap().as_slice(),
            &[Snapshot::new(1, TestState::Present { enabled: true })]
        );
    }

    #[test]
    fn frequency_snapshot_saves_on_matching_revision() {
        const EVERY_THREE_REVISIONS: NonZeroU64 = NonZeroU64::new(3).expect("snapshot cadence must be non-zero");
        let runtime = FakeRuntime {
            next_expected_version: 3,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(Snapshots::new(
                    &runtime,
                    test_snapshot_config(),
                    FrequencySnapshot::new(EVERY_THREE_REVISIONS),
                ))
                .execute_result(),
        )
        .unwrap();

        assert_eq!(
            runtime.saved_snapshots.lock().unwrap().as_slice(),
            &[Snapshot::new(3, TestState::Present { enabled: true })]
        );
    }

    #[test]
    fn frequency_snapshot_skips_non_matching_revision() {
        const EVERY_THREE_REVISIONS: NonZeroU64 = NonZeroU64::new(3).expect("snapshot cadence must be non-zero");
        let runtime = FakeRuntime {
            next_expected_version: 2,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(Snapshots::new(
                    &runtime,
                    test_snapshot_config(),
                    FrequencySnapshot::new(EVERY_THREE_REVISIONS),
                ))
                .execute_result(),
        )
        .unwrap();

        assert!(runtime.saved_snapshots.lock().unwrap().is_empty());
    }

    #[test]
    fn does_not_save_snapshot_when_policy_skips_it() {
        let runtime = FakeRuntime {
            next_expected_version: 1,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(Snapshots::new(&runtime, test_snapshot_config(), NoSnapshot))
                .execute_result(),
        )
        .unwrap();

        assert!(runtime.saved_snapshots.lock().unwrap().is_empty());
    }

    #[test]
    fn propagates_snapshot_save_failures() {
        let runtime = FakeRuntime {
            next_expected_version: 1,
            fail_save_snapshot: true,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let error = block_on(
            CommandExecution::new(&runtime, &command)
                .with_snapshot(Snapshots::new(
                    &runtime,
                    test_snapshot_config(),
                    FrequencySnapshot::new(NonZeroU64::MIN),
                ))
                .execute_result(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Infra(CommandInfraError::SaveSnapshot(TestInfraError::SaveSnapshot))
        ));
    }

    #[test]
    fn resolves_stream_id_once() {
        let runtime = FakeRuntime {
            next_expected_version: 1,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(CommandExecution::new(&runtime, &command).execute_result()).unwrap();

        assert_eq!(command.stream_id_calls(), 1);
        assert!(runtime.loaded_stream_ids.lock().unwrap().is_empty());
    }
}
