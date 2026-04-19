use crate::{
    Decide, Decision, EventCodec, EventData, EventType, JsonEventCodec, NonEmpty, RecordedEvent,
    Snapshot, SnapshotStoreConfig, StreamCommand, StreamEvent,
};

pub trait CommandState: StreamCommand + Sized {
    type State;
    type Event;
    type DomainError;

    fn initial_state() -> Self::State;

    fn evolve(state: Self::State, event: Self::Event) -> Result<Self::State, Self::DomainError>;
}

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
pub enum CommandStreamState {
    Prefer(StreamState),
    Require(StreamState),
}

pub const fn resolve_stream_state(
    occ: Option<OccPolicy>,
    current_version: Option<u64>,
    command_stream_state: Option<CommandStreamState>,
) -> StreamState {
    if let Some(CommandStreamState::Require(stream_state)) = command_stream_state {
        return stream_state;
    }

    match occ {
        Some(OccPolicy::ExactCurrentVersion) => StreamState::from_current_version(current_version),
        Some(OccPolicy::Explicit(stream_state)) => stream_state,
        None => match command_stream_state {
            Some(CommandStreamState::Prefer(stream_state)) => stream_state,
            Some(CommandStreamState::Require(stream_state)) => stream_state,
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

pub trait SnapshotStore<SnapshotPayload, StreamId: ?Sized>: Send + Sync {
    type Error;

    fn load_snapshot(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &StreamId,
    ) -> impl std::future::Future<Output = Result<Option<Snapshot<SnapshotPayload>>, Self::Error>> + Send;

    fn save_snapshot(
        &self,
        config: SnapshotStoreConfig<'static>,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AlwaysSnapshot;

impl<State, Event> SnapshotPolicy<State, Event> for AlwaysSnapshot {
    fn snapshot_decision(
        &self,
        _next_expected_version: u64,
        _state: &State,
        _events: &NonEmpty<Event>,
    ) -> SnapshotDecision {
        SnapshotDecision::Take
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

pub type CommandResult<State, Event, DomainError, InfraError> = Result<
    ExecutionResult<State, Event>,
    CommandFailure<DomainError, CommandInfraError<InfraError>>,
>;

#[derive(Debug)]
pub enum CommandFailure<DomainError, InfraError> {
    Domain(DomainError),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshots<'a, S, P> {
    snapshot_store: &'a S,
    snapshot_config: SnapshotStoreConfig<'static>,
    policy: P,
}

impl<'a, S, P> Snapshots<'a, S, P> {
    pub const fn new(
        snapshot_store: &'a S,
        snapshot_config: SnapshotStoreConfig<'static>,
        policy: P,
    ) -> Self {
        Self {
            snapshot_store,
            snapshot_config,
            policy,
        }
    }
}

pub struct CommandExecution<'a, E, C, S = WithoutSnapshots> {
    event_store: &'a E,
    command: &'a C,
    occ: Option<OccPolicy>,
    snapshots: S,
    event_codec: JsonEventCodec,
}

impl<'a, E, C> CommandExecution<'a, E, C, WithoutSnapshots> {
    pub const fn new(event_store: &'a E, command: &'a C) -> Self {
        Self {
            event_store,
            command,
            occ: None,
            snapshots: WithoutSnapshots,
            event_codec: JsonEventCodec,
        }
    }

    pub fn snapshots<S, P>(
        self,
        snapshots: Snapshots<'a, S, P>,
    ) -> CommandExecution<'a, E, C, Snapshots<'a, S, P>> {
        CommandExecution {
            event_store: self.event_store,
            command: self.command,
            occ: self.occ,
            snapshots,
            event_codec: self.event_codec,
        }
    }
}

impl<'a, E, C, S> CommandExecution<'a, E, C, S> {
    pub fn occ<O>(mut self, occ: O) -> Self
    where
        O: Into<Option<OccPolicy>>,
    {
        self.occ = occ.into();
        self
    }
}

impl<'a, E, C, S> CommandExecution<'a, E, C, S> {
    pub fn codec<EC>(self, event_codec: EC) -> CommandExecutionWithCodec<'a, E, C, S, EC> {
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

impl<'a, E, C, S, EC> CommandExecutionWithCodec<'a, E, C, S, EC> {
    pub fn occ<O>(mut self, occ: O) -> Self
    where
        O: Into<Option<OccPolicy>>,
    {
        self.occ = occ.into();
        self
    }

    pub fn snapshots<S2, P>(
        self,
        snapshots: Snapshots<'a, S2, P>,
    ) -> CommandExecutionWithCodec<'a, E, C, Snapshots<'a, S2, P>, EC> {
        CommandExecutionWithCodec {
            event_store: self.event_store,
            command: self.command,
            occ: self.occ,
            snapshots,
            event_codec: self.event_codec,
        }
    }
}

impl<E, C, SErr> CommandExecution<'_, E, C, WithoutSnapshots>
where
    C: CommandState + Decide<C::State, C::Event>,
    C::Event: EventType + StreamEvent + Clone,
    E: StreamRead<C::StreamId, Error = SErr> + StreamAppend<C::StreamId, Error = SErr>,
    C::DomainError: From<C::Error>,
    JsonEventCodec: EventCodec<C::Event>,
    <JsonEventCodec as EventCodec<C::Event>>::Error: Into<SErr>,
{
    pub async fn execute(
        self,
    ) -> Result<
        ExecutionResult<C::State, C::Event>,
        CommandFailure<C::DomainError, CommandInfraError<SErr>>,
    > {
        self.execute_result().await
    }

    async fn execute_result(
        self,
    ) -> Result<
        ExecutionResult<C::State, C::Event>,
        CommandFailure<C::DomainError, CommandInfraError<SErr>>,
    > {
        let event_codec = self.event_codec;
        self.codec(event_codec).execute_result().await
    }
}

impl<E, C, EC, SErr> CommandExecutionWithCodec<'_, E, C, WithoutSnapshots, EC>
where
    C: CommandState + Decide<C::State, C::Event>,
    C::Event: EventType + StreamEvent + Clone,
    E: StreamRead<C::StreamId, Error = SErr> + StreamAppend<C::StreamId, Error = SErr>,
    C::DomainError: From<C::Error>,
    EC: EventCodec<C::Event>,
    EC::Error: Into<SErr>,
{
    pub async fn execute(
        self,
    ) -> Result<
        ExecutionResult<C::State, C::Event>,
        CommandFailure<C::DomainError, CommandInfraError<SErr>>,
    > {
        self.execute_result().await
    }

    async fn execute_result(
        self,
    ) -> Result<
        ExecutionResult<C::State, C::Event>,
        CommandFailure<C::DomainError, CommandInfraError<SErr>>,
    > {
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
                .map_err(Into::into)
                .map_err(CommandInfraError::DecodeEvent)
                .map_err(CommandFailure::Infra)?;
            state = C::evolve(state, event).map_err(CommandFailure::Domain)?;
        }

        let Decision::Event(events) = C::decide(&state, self.command)
            .map_err(C::DomainError::from)
            .map_err(CommandFailure::Domain)?;
        let encoded_events = encode_events(&self.event_codec, &events)
            .map_err(Into::into)
            .map_err(CommandInfraError::EncodeEvent)
            .map_err(CommandFailure::Infra)?;
        let stream_state =
            resolve_stream_state(self.occ, current_version, self.command.stream_state());
        let append_outcome = self
            .event_store
            .append_events(stream_id, stream_state, encoded_events)
            .await
            .map_err(CommandInfraError::Append)
            .map_err(CommandFailure::Infra)?;

        for event in events.iter().cloned() {
            state = C::evolve(state, event).map_err(CommandFailure::Domain)?;
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
    C: CommandState + Decide<C::State, C::Event>,
    C::Event: EventType + StreamEvent + Clone,
    C::State: Clone,
    E: StreamRead<C::StreamId, Error = SErr> + StreamAppend<C::StreamId, Error = SErr>,
    S: SnapshotStore<C::State, C::StreamId, Error = SErr>,
    P: SnapshotPolicy<C::State, C::Event>,
    C::DomainError: From<C::Error>,
    JsonEventCodec: EventCodec<C::Event>,
    <JsonEventCodec as EventCodec<C::Event>>::Error: Into<SErr>,
{
    pub async fn execute(
        self,
    ) -> Result<
        ExecutionResult<C::State, C::Event>,
        CommandFailure<C::DomainError, CommandInfraError<SErr>>,
    > {
        self.execute_result().await
    }

    async fn execute_result(
        self,
    ) -> Result<
        ExecutionResult<C::State, C::Event>,
        CommandFailure<C::DomainError, CommandInfraError<SErr>>,
    > {
        let event_codec = self.event_codec;
        self.codec(event_codec).execute_result().await
    }
}

impl<E, S, C, P, SErr, EC> CommandExecutionWithCodec<'_, E, C, Snapshots<'_, S, P>, EC>
where
    C: CommandState + Decide<C::State, C::Event>,
    C::Event: EventType + StreamEvent + Clone,
    C::State: Clone,
    E: StreamRead<C::StreamId, Error = SErr> + StreamAppend<C::StreamId, Error = SErr>,
    S: SnapshotStore<C::State, C::StreamId, Error = SErr>,
    P: SnapshotPolicy<C::State, C::Event>,
    C::DomainError: From<C::Error>,
    EC: EventCodec<C::Event>,
    EC::Error: Into<SErr>,
{
    pub async fn execute(
        self,
    ) -> Result<
        ExecutionResult<C::State, C::Event>,
        CommandFailure<C::DomainError, CommandInfraError<SErr>>,
    > {
        self.execute_result().await
    }

    async fn execute_result(
        self,
    ) -> Result<
        ExecutionResult<C::State, C::Event>,
        CommandFailure<C::DomainError, CommandInfraError<SErr>>,
    > {
        let stream_id = self.command.stream_id();
        let snapshot = self
            .snapshots
            .snapshot_store
            .load_snapshot(self.snapshots.snapshot_config, stream_id)
            .await
            .map_err(CommandInfraError::LoadSnapshot)
            .map_err(CommandFailure::Infra)?;
        let snapshot_version = snapshot.as_ref().map(|snapshot| snapshot.version);
        let mut state = snapshot
            .map(|snapshot| snapshot.payload)
            .unwrap_or_else(C::initial_state);
        let start_sequence = snapshot_version
            .map(|version| version.saturating_add(1))
            .unwrap_or(1);
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
                    return Err(CommandFailure::Infra(
                        CommandInfraError::SnapshotAheadOfStream {
                            snapshot_version,
                            stream_version,
                        },
                    ));
                }
            }
        }

        for recorded_event in stream_read.events {
            let event = recorded_event
                .decode_data_with(&self.event_codec)
                .map_err(Into::into)
                .map_err(CommandInfraError::DecodeEvent)
                .map_err(CommandFailure::Infra)?;
            state = C::evolve(state, event).map_err(CommandFailure::Domain)?;
        }

        let Decision::Event(events) = C::decide(&state, self.command)
            .map_err(C::DomainError::from)
            .map_err(CommandFailure::Domain)?;
        let encoded_events = encode_events(&self.event_codec, &events)
            .map_err(Into::into)
            .map_err(CommandInfraError::EncodeEvent)
            .map_err(CommandFailure::Infra)?;
        let stream_state =
            resolve_stream_state(self.occ, current_version, self.command.stream_state());
        let append_outcome = self
            .event_store
            .append_events(stream_id, stream_state, encoded_events)
            .await
            .map_err(CommandInfraError::Append)
            .map_err(CommandFailure::Infra)?;

        for event in events.iter().cloned() {
            state = C::evolve(state, event).map_err(CommandFailure::Domain)?;
        }

        if matches!(
            self.snapshots.policy.snapshot_decision(
                append_outcome.next_expected_version,
                &state,
                &events,
            ),
            SnapshotDecision::Take
        ) {
            self.snapshots
                .snapshot_store
                .save_snapshot(
                    self.snapshots.snapshot_config,
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
    events
        .clone()
        .try_map(|event| EventData::new_with_codec(codec, event))
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
    use crate::{EventType, StreamEvent};

    #[derive(Debug, Clone)]
    struct TestCommand {
        id: String,
        action: TestAction,
        stream_state: Option<CommandStreamState>,
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
                stream_state: None,
                stream_id_calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn with_default_stream_state(mut self, stream_state: StreamState) -> Self {
            self.stream_state = Some(CommandStreamState::Prefer(stream_state));
            self
        }

        fn with_required_stream_state(mut self, stream_state: StreamState) -> Self {
            self.stream_state = Some(CommandStreamState::Require(stream_state));
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

        fn stream_state(&self) -> Option<CommandStreamState> {
            self.stream_state
        }
    }

    impl CommandState for TestCommand {
        type State = TestState;
        type Event = TestEvent;
        type DomainError = TestCommandError;

        fn initial_state() -> Self::State {
            TestState::Missing
        }

        fn evolve(
            _state: Self::State,
            event: Self::Event,
        ) -> Result<Self::State, Self::DomainError> {
            match event {
                TestEvent::Registered { .. } => Ok(TestState::Present { enabled: true }),
                TestEvent::StateChanged { enabled, .. } => Ok(TestState::Present { enabled }),
                TestEvent::Removed { .. } => Ok(TestState::Missing),
                TestEvent::Broken { .. } => Err(TestCommandError::BrokenEvent),
            }
        }
    }

    impl Decide<TestState, TestEvent> for TestCommand {
        type Error = TestDecisionError;

        fn decide(state: &TestState, command: &Self) -> Result<Decision<TestEvent>, Self::Error> {
            match (state, command.action) {
                (TestState::Missing, TestAction::Register) => {
                    Ok(Decision::Event(NonEmpty::one(TestEvent::Registered {
                        id: command.id.clone(),
                    })))
                }
                (TestState::Present { .. }, TestAction::Register) => {
                    Err(TestDecisionError::AlreadyRegistered)
                }
                (TestState::Present { enabled: false }, TestAction::Disable) => {
                    Err(TestDecisionError::AlreadyDisabled)
                }
                (TestState::Present { .. }, TestAction::Disable) => {
                    Ok(Decision::Event(NonEmpty::one(TestEvent::StateChanged {
                        id: command.id.clone(),
                        enabled: false,
                    })))
                }
                (TestState::Missing, TestAction::Disable | TestAction::Remove) => {
                    Err(TestDecisionError::Missing)
                }
                (TestState::Present { .. }, TestAction::Remove) => {
                    Ok(Decision::Event(NonEmpty::one(TestEvent::Removed {
                        id: command.id.clone(),
                    })))
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

    const TEST_SNAPSHOT_CONFIG: SnapshotStoreConfig<'static> =
        SnapshotStoreConfig::new("test.command.", None);

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
            self.appended_events
                .lock()
                .unwrap()
                .extend(events.into_vec());
            Ok(AppendOutcome {
                next_expected_version: self.next_expected_version,
            })
        }
    }

    impl SnapshotStore<TestState, str> for FakeRuntime {
        type Error = TestInfraError;

        async fn load_snapshot(
            &self,
            _config: SnapshotStoreConfig<'static>,
            stream_id: &str,
        ) -> Result<Option<Snapshot<TestState>>, Self::Error> {
            if self.fail_load_snapshot {
                return Err(TestInfraError::LoadSnapshot);
            }
            self.loaded_stream_ids
                .lock()
                .unwrap()
                .push(stream_id.to_string());
            Ok(self.snapshot.clone())
        }

        async fn save_snapshot(
            &self,
            _config: SnapshotStoreConfig<'static>,
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
                .snapshots(Snapshots::new(&runtime, TEST_SNAPSHOT_CONFIG, NoSnapshot))
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
                .snapshots(Snapshots::new(&runtime, TEST_SNAPSHOT_CONFIG, NoSnapshot))
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
                .snapshots(Snapshots::new(&runtime, TEST_SNAPSHOT_CONFIG, NoSnapshot))
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

        let error =
            block_on(CommandExecution::new(&runtime, &command).execute_result()).unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Domain(TestCommandError::BrokenEvent)
        ));
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
                .snapshots(Snapshots::new(&runtime, TEST_SNAPSHOT_CONFIG, NoSnapshot))
                .execute_result(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Domain(TestCommandError::AlreadyRegistered)
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
                .snapshots(Snapshots::new(&runtime, TEST_SNAPSHOT_CONFIG, NoSnapshot))
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
                .codec(WrappedJsonCodec)
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
            .with_default_stream_state(StreamState::StreamExists);

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
            .with_default_stream_state(StreamState::StreamExists);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .occ(OccPolicy::Explicit(StreamState::Any))
                .execute_result(),
        )
        .unwrap();

        assert_eq!(
            runtime.stream_states.lock().unwrap().as_slice(),
            &[StreamState::Any]
        );
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
        let command = TestCommand::new("alpha", TestAction::Remove)
            .with_default_stream_state(StreamState::StreamExists);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .occ(OccPolicy::ExactCurrentVersion)
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
        let command = TestCommand::new("alpha", TestAction::Register)
            .with_required_stream_state(StreamState::NoStream);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .occ(OccPolicy::Explicit(StreamState::Any))
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
                .snapshots(Snapshots::new(
                    &runtime,
                    TEST_SNAPSHOT_CONFIG,
                    AlwaysSnapshot,
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
        let runtime = FakeRuntime {
            next_expected_version: 3,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .snapshots(Snapshots::new(
                    &runtime,
                    TEST_SNAPSHOT_CONFIG,
                    FrequencySnapshot::new(NonZeroU64::new(3).unwrap()),
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
        let runtime = FakeRuntime {
            next_expected_version: 2,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .snapshots(Snapshots::new(
                    &runtime,
                    TEST_SNAPSHOT_CONFIG,
                    FrequencySnapshot::new(NonZeroU64::new(3).unwrap()),
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
                .snapshots(Snapshots::new(&runtime, TEST_SNAPSHOT_CONFIG, NoSnapshot))
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
                .snapshots(Snapshots::new(
                    &runtime,
                    TEST_SNAPSHOT_CONFIG,
                    AlwaysSnapshot,
                ))
                .execute_result(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Infra(CommandInfraError::SaveSnapshot(
                TestInfraError::SaveSnapshot
            ))
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
