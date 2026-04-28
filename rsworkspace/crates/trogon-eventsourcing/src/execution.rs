use crate::snapshot::{Snapshot, SnapshotRead, SnapshotSchema, SnapshotStoreConfig, SnapshotWrite};
use crate::stream::{StreamAppend, StreamRead, StreamState, resolve_stream_state};
use crate::{CanonicalEventCodec, Decide, Decision, EventCodec, EventData, EventEnvelopeCodec, NonEmpty};

use std::num::NonZeroU64;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotDecision {
    Skip,
    Take,
}

#[derive(Debug, Clone, Copy)]
pub struct SnapshotDecisionContext<'a, State, Event> {
    pub next_expected_version: u64,
    pub state: &'a State,
    pub events: &'a NonEmpty<Event>,
}

pub trait SnapshotPolicy<State, Event> {
    fn snapshot_decision(&self, context: SnapshotDecisionContext<'_, State, Event>) -> SnapshotDecision;
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
        if context.next_expected_version.is_multiple_of(self.frequency.get()) {
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

pub type CommandResult<C, InfraError> = Result<
    ExecutionResult<<C as Decide>::State, <C as Decide>::Event>,
    CommandFailure<<C as Decide>::DecideError, <C as Decide>::EvolveError, CommandInfraError<InfraError>>,
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
    EncodeEvent(BoxError),
    DecodeEvent(BoxError),
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
    write_precondition: Option<StreamState>,
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
            write_precondition: None,
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
            write_precondition: self.write_precondition,
            snapshots: snapshots.into_snapshots(),
        }
    }
}

impl<'a, E, C, S> CommandExecution<'a, E, C, S> {
    pub fn with_write_precondition<W>(mut self, write_precondition: W) -> Self
    where
        W: Into<Option<StreamState>>,
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
    write_precondition: Option<StreamState>,
    snapshots: S,
    event_codec: EC,
}

impl<'a, E, C, S, EC> CommandExecutionWithCodec<'a, E, C, S, EC>
where
    C: Decide,
{
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
            write_precondition: self.write_precondition,
            snapshots: snapshots.into_snapshots(),
            event_codec: self.event_codec,
        }
    }
}

impl<'a, E, C, S, EC> CommandExecutionWithCodec<'a, E, C, S, EC> {
    pub fn with_write_precondition<W>(mut self, write_precondition: W) -> Self
    where
        W: Into<Option<StreamState>>,
    {
        self.write_precondition = write_precondition.into();
        self
    }
}

impl<E, C, SErr> CommandExecution<'_, E, C, WithoutSnapshots>
where
    C: Decide,
    C::Event: Clone + CanonicalEventCodec,
    C::StreamId: AsRef<str>,
    E: StreamRead<C::StreamId, Error = SErr> + StreamAppend<C::StreamId, Error = SErr>,
    <C::Event as CanonicalEventCodec>::Codec: EventEnvelopeCodec<C::Event>,
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
    C: Decide,
    C::Event: Clone,
    C::StreamId: AsRef<str>,
    E: StreamRead<C::StreamId, Error = SErr> + StreamAppend<C::StreamId, Error = SErr>,
    EC: EventEnvelopeCodec<C::Event>,
    EC::Error: std::error::Error + Send + Sync + 'static,
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
        let mut state = C::initial_state();

        for recorded_event in stream_read.events {
            let event = recorded_event
                .decode_data_with(&self.event_codec)
                .map_err(box_error)
                .map_err(CommandInfraError::DecodeEvent)
                .map_err(CommandFailure::Infra)?;
            state = C::evolve(state, &event).map_err(CommandFailure::Evolve)?;
        }

        let Decision::Event(events) = C::decide(&state, self.command).map_err(CommandFailure::Decide)?;
        let encoded_events = encode_events(stream_id.as_ref(), &self.event_codec, &events)
            .map_err(box_error)
            .map_err(CommandInfraError::EncodeEvent)
            .map_err(CommandFailure::Infra)?;
        let stream_state = resolve_stream_state::<C>(self.write_precondition, current_version);
        let append_outcome = self
            .event_store
            .append_events(stream_id, stream_state, encoded_events)
            .await
            .map_err(CommandInfraError::Append)
            .map_err(CommandFailure::Infra)?;

        for event in events.iter() {
            state = C::evolve(state, event).map_err(CommandFailure::Evolve)?;
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
    C::Event: Clone + CanonicalEventCodec,
    C::StreamId: AsRef<str>,
    E: StreamRead<C::StreamId, Error = SErr> + StreamAppend<C::StreamId, Error = SErr>,
    S: SnapshotRead<C::State, C::StreamId, Error = SErr> + SnapshotWrite<C::State, C::StreamId, Error = SErr>,
    P: SnapshotPolicy<C::State, C::Event>,
    <C::Event as CanonicalEventCodec>::Codec: EventEnvelopeCodec<C::Event>,
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

impl<E, S, C, P, SErr, EC> CommandExecutionWithCodec<'_, E, C, Snapshots<'_, S, P>, EC>
where
    C: Decide,
    C::State: Clone,
    C::Event: Clone,
    C::StreamId: AsRef<str>,
    E: StreamRead<C::StreamId, Error = SErr> + StreamAppend<C::StreamId, Error = SErr>,
    S: SnapshotRead<C::State, C::StreamId, Error = SErr> + SnapshotWrite<C::State, C::StreamId, Error = SErr>,
    P: SnapshotPolicy<C::State, C::Event>,
    EC: EventEnvelopeCodec<C::Event>,
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
            .load_snapshot(self.snapshots.snapshot_config.clone(), stream_id)
            .await
            .map_err(CommandInfraError::LoadSnapshot)
            .map_err(CommandFailure::Infra)?;
        let snapshot_version = snapshot.as_ref().map(|snapshot| snapshot.version);
        let mut state = snapshot
            .map(|snapshot| snapshot.payload)
            .unwrap_or_else(C::initial_state);
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
                .map_err(box_error)
                .map_err(CommandInfraError::DecodeEvent)
                .map_err(CommandFailure::Infra)?;
            state = C::evolve(state, &event).map_err(CommandFailure::Evolve)?;
        }

        let Decision::Event(events) = C::decide(&state, self.command).map_err(CommandFailure::Decide)?;
        let encoded_events = encode_events(stream_id.as_ref(), &self.event_codec, &events)
            .map_err(box_error)
            .map_err(CommandInfraError::EncodeEvent)
            .map_err(CommandFailure::Infra)?;
        let stream_state = resolve_stream_state::<C>(self.write_precondition, current_version);
        let append_outcome = self
            .event_store
            .append_events(stream_id, stream_state, encoded_events)
            .await
            .map_err(CommandInfraError::Append)
            .map_err(CommandFailure::Infra)?;

        for event in events.iter() {
            state = C::evolve(state, event).map_err(CommandFailure::Evolve)?;
        }

        if matches!(
            self.snapshots.policy.snapshot_decision(SnapshotDecisionContext {
                next_expected_version: append_outcome.next_expected_version,
                state: &state,
                events: &events,
            }),
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

fn box_error<E>(error: E) -> BoxError
where
    E: std::error::Error + Send + Sync + 'static,
{
    Box::new(error)
}

fn encode_events<E, C>(stream_id: &str, codec: &C, events: &NonEmpty<E>) -> Result<NonEmpty<EventData>, C::Error>
where
    E: Clone,
    C: EventEnvelopeCodec<E>,
{
    events
        .clone()
        .try_map(|event| EventData::new_with_codec(stream_id, codec, event))
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
        CanonicalEventCodec, EventEnvelopeCodec, EventIdentity, EventType, RecordedEvent, SnapshotSchema,
        StreamReadResult, stream::AppendOutcome,
    };

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

    impl Decide for TestCommand {
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

    impl Decide for RequiredRegisterCommand {
        type StreamId = str;
        type State = TestState;
        type Event = TestEvent;
        type DecideError = TestDecisionError;
        type EvolveError = TestCommandError;

        const REQUIRED_WRITE_PRECONDITION: Option<StreamState> = Some(StreamState::NoStream);

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

        fn decide(state: &TestState, command: &Self) -> Result<Decision<TestEvent>, Self::DecideError> {
            match state {
                TestState::Missing => Ok(Decision::event(TestEvent::Registered { id: command.id.clone() })),
                TestState::Present { .. } => Err(TestDecisionError::AlreadyRegistered),
            }
        }
    }

    impl EventIdentity for TestEvent {}

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

    impl EventEnvelopeCodec<TestEvent> for WrappedJsonCodec {
        fn event_type(&self, value: &TestEvent) -> Result<&'static str, Self::Error> {
            Ok(value.event_type())
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
        let stream_id = match &event {
            TestEvent::Registered { id }
            | TestEvent::StateChanged { id, .. }
            | TestEvent::Removed { id }
            | TestEvent::Broken { id } => id.clone(),
        };
        EventData::new(stream_id, event).unwrap().record(
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
                    "alpha",
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
        assert!(appended_events[0].payload.starts_with(b"wrapped:"));
    }

    #[test]
    fn falls_back_to_exact_current_version_when_command_has_no_required_rule() {
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
        let command = TestCommand::new("alpha", TestAction::Remove);

        let _ = block_on(CommandExecution::new(&runtime, &command).execute_result()).unwrap();

        assert_eq!(
            runtime.stream_states.lock().unwrap().as_slice(),
            &[StreamState::StreamRevision(7)]
        );
    }

    #[test]
    fn explicit_write_precondition_overrides_exact_current_version_fallback() {
        let runtime = FakeRuntime {
            next_expected_version: 1,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .with_write_precondition(StreamState::Any)
                .execute_result(),
        )
        .unwrap();

        assert_eq!(runtime.stream_states.lock().unwrap().as_slice(), &[StreamState::Any]);
    }

    #[test]
    fn required_command_rule_uses_required_stream_state() {
        let runtime = FakeRuntime {
            next_expected_version: 1,
            ..Default::default()
        };
        let command = RequiredRegisterCommand::new("alpha");

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .with_write_precondition(StreamState::Any)
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
