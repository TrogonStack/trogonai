use serde::{Serialize, de::DeserializeOwned};

use crate::{
    Decide, Decision, EventData, EventType, NonEmpty, RecordedEvent, Snapshot, SnapshotStoreConfig,
    StreamCommand, StreamEvent,
};

pub trait CommandStateModel: StreamCommand + Sized {
    type State;
    type Event;
    type DomainError;

    fn initial_state() -> Self::State;

    fn evolve(state: Self::State, event: Self::Event) -> Result<Self::State, Self::DomainError>;
}

pub trait SnapshotStateModel: CommandStateModel {
    type Snapshot;

    fn restore_state(
        _command: &Self,
        snapshot: Option<Snapshot<Self::Snapshot>>,
    ) -> Result<Self::State, Self::DomainError>
    where
        Self::Snapshot: Into<Self::State>,
    {
        Ok(snapshot
            .map(|snapshot| snapshot.payload.into())
            .unwrap_or_else(Self::initial_state))
    }

    fn snapshot_state(state: &Self::State) -> Option<Self::Snapshot>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedState {
    Any,
    StreamExists,
    NoStream,
    StreamRevision(u64),
}

impl ExpectedState {
    pub const fn from_current_version(current_version: Option<u64>) -> Self {
        match current_version {
            Some(version) => Self::StreamRevision(version),
            None => Self::NoStream,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OccPolicy {
    #[default]
    CommandDefault,
    ExactCurrentVersion,
    Explicit(ExpectedState),
}

impl OccPolicy {
    pub const fn resolve(
        self,
        current_version: Option<u64>,
        command_default: Option<ExpectedState>,
    ) -> ExpectedState {
        match self {
            Self::CommandDefault => match command_default {
                Some(expected_state) => expected_state,
                None => ExpectedState::from_current_version(current_version),
            },
            Self::ExactCurrentVersion => ExpectedState::from_current_version(current_version),
            Self::Explicit(expected_state) => expected_state,
        }
    }
}

pub trait DefaultExpectedStateProvider: StreamCommand {
    fn default_expected_state(&self) -> Option<ExpectedState> {
        None
    }
}

pub trait EventStore<StreamId: ?Sized>: Send + Sync {
    type Error;

    fn current_stream_version(
        &self,
        stream_id: &StreamId,
    ) -> impl std::future::Future<Output = Result<Option<u64>, Self::Error>> + Send;

    fn read_stream_from(
        &self,
        stream_id: &StreamId,
        from_sequence: u64,
    ) -> impl std::future::Future<Output = Result<Vec<RecordedEvent>, Self::Error>> + Send;

    fn append_events(
        &self,
        stream_id: &StreamId,
        expected_state: ExpectedState,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendOutcome {
    pub next_expected_version: u64,
}

pub trait SnapshotPolicy<State, Event> {
    fn should_snapshot(
        &self,
        next_expected_version: u64,
        state: &State,
        events: &NonEmpty<Event>,
    ) -> bool;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AlwaysSnapshot;

impl<State, Event> SnapshotPolicy<State, Event> for AlwaysSnapshot {
    fn should_snapshot(
        &self,
        _next_expected_version: u64,
        _state: &State,
        _events: &NonEmpty<Event>,
    ) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoSnapshot;

impl<State, Event> SnapshotPolicy<State, Event> for NoSnapshot {
    fn should_snapshot(
        &self,
        _next_expected_version: u64,
        _state: &State,
        _events: &NonEmpty<Event>,
    ) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionResult<State, Event> {
    pub next_expected_version: u64,
    pub events: NonEmpty<Event>,
    pub state: State,
}

#[derive(Debug)]
pub enum ExecuteError<DecisionError, DomainError, RuntimeError> {
    LoadSnapshot(RuntimeError),
    SaveSnapshot(RuntimeError),
    ReadStream(RuntimeError),
    Append(RuntimeError),
    EncodeEvent(serde_json::Error),
    DecodeEvent(serde_json::Error),
    Decision(DecisionError),
    Domain(DomainError),
    SnapshotAheadOfStream {
        snapshot_version: u64,
        stream_version: Option<u64>,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WithoutSnapshots;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WithSnapshots<'a, S, P> {
    snapshot_store: &'a S,
    snapshot_config: SnapshotStoreConfig<'static>,
    policy: P,
}

pub struct CommandExecution<'a, E, C, S = WithoutSnapshots> {
    event_store: &'a E,
    command: &'a C,
    occ: OccPolicy,
    snapshots: S,
}

impl<'a, E, C> CommandExecution<'a, E, C, WithoutSnapshots> {
    pub const fn new(event_store: &'a E, command: &'a C) -> Self {
        Self {
            event_store,
            command,
            occ: OccPolicy::CommandDefault,
            snapshots: WithoutSnapshots,
        }
    }

    pub fn snapshots<S, P>(
        self,
        snapshot_store: &'a S,
        snapshot_config: SnapshotStoreConfig<'static>,
        policy: P,
    ) -> CommandExecution<'a, E, C, WithSnapshots<'a, S, P>> {
        CommandExecution {
            event_store: self.event_store,
            command: self.command,
            occ: self.occ,
            snapshots: WithSnapshots {
                snapshot_store,
                snapshot_config,
                policy,
            },
        }
    }
}

impl<E, C, S> CommandExecution<'_, E, C, S> {
    pub fn occ(mut self, occ: OccPolicy) -> Self {
        self.occ = occ;
        self
    }
}

impl<E, C> CommandExecution<'_, E, C, WithoutSnapshots>
where
    C: CommandStateModel + Decide<C::State, C::Event> + DefaultExpectedStateProvider,
    C::Event: EventType + StreamEvent + Serialize + DeserializeOwned + Clone,
    E: EventStore<C::StreamId>,
{
    pub async fn execute(
        self,
    ) -> Result<ExecutionResult<C::State, C::Event>, ExecuteError<C::Error, C::DomainError, E::Error>>
    {
        let stream_id = self.command.stream_id();
        let current_version = self
            .event_store
            .current_stream_version(stream_id)
            .await
            .map_err(ExecuteError::ReadStream)?;
        let mut state = C::initial_state();

        if let Some(current_version) = current_version {
            let recorded_events = self
                .event_store
                .read_stream_from(stream_id, 1)
                .await
                .map_err(ExecuteError::ReadStream)?;

            for recorded_event in recorded_events
                .into_iter()
                .filter(|event| event.log_position.unwrap_or(0) <= current_version)
            {
                let event = recorded_event
                    .decode_data::<C::Event>()
                    .map_err(ExecuteError::DecodeEvent)?;
                state = C::evolve(state, event).map_err(ExecuteError::Domain)?;
            }
        }

        let Decision::Event(events) =
            C::decide(&state, self.command).map_err(ExecuteError::Decision)?;
        let encoded_events = encode_events(&events).map_err(ExecuteError::EncodeEvent)?;
        let expected_state = self
            .occ
            .resolve(current_version, self.command.default_expected_state());
        let append_outcome = self
            .event_store
            .append_events(stream_id, expected_state, encoded_events)
            .await
            .map_err(ExecuteError::Append)?;

        for event in events.iter().cloned() {
            state = C::evolve(state, event).map_err(ExecuteError::Domain)?;
        }

        Ok(ExecutionResult {
            next_expected_version: append_outcome.next_expected_version,
            events,
            state,
        })
    }
}

impl<E, S, C, P, SErr> CommandExecution<'_, E, C, WithSnapshots<'_, S, P>>
where
    C: SnapshotStateModel + Decide<C::State, C::Event> + DefaultExpectedStateProvider,
    C::Event: EventType + StreamEvent + Serialize + DeserializeOwned + Clone,
    C::Snapshot: Into<C::State>,
    E: EventStore<C::StreamId, Error = SErr>,
    S: SnapshotStore<C::Snapshot, C::StreamId, Error = SErr>,
    P: SnapshotPolicy<C::State, C::Event>,
{
    pub async fn execute(
        self,
    ) -> Result<ExecutionResult<C::State, C::Event>, ExecuteError<C::Error, C::DomainError, SErr>>
    {
        let stream_id = self.command.stream_id();
        let snapshot = self
            .snapshots
            .snapshot_store
            .load_snapshot(self.snapshots.snapshot_config, stream_id)
            .await
            .map_err(ExecuteError::LoadSnapshot)?;
        let snapshot_version = snapshot.as_ref().map(|snapshot| snapshot.version);
        let mut state = C::restore_state(self.command, snapshot).map_err(ExecuteError::Domain)?;
        let current_version = self
            .event_store
            .current_stream_version(stream_id)
            .await
            .map_err(ExecuteError::ReadStream)?;

        if let Some(snapshot_version) = snapshot_version {
            match current_version {
                Some(stream_version) if snapshot_version <= stream_version => {}
                stream_version => {
                    return Err(ExecuteError::SnapshotAheadOfStream {
                        snapshot_version,
                        stream_version,
                    });
                }
            }
        }

        if let Some(current_version) = current_version {
            let start_sequence = snapshot_version
                .map(|version| version.saturating_add(1))
                .unwrap_or(1);

            if start_sequence <= current_version {
                let recorded_events = self
                    .event_store
                    .read_stream_from(stream_id, start_sequence)
                    .await
                    .map_err(ExecuteError::ReadStream)?;

                for recorded_event in recorded_events {
                    let event = recorded_event
                        .decode_data::<C::Event>()
                        .map_err(ExecuteError::DecodeEvent)?;
                    state = C::evolve(state, event).map_err(ExecuteError::Domain)?;
                }
            }
        }

        let Decision::Event(events) =
            C::decide(&state, self.command).map_err(ExecuteError::Decision)?;
        let encoded_events = encode_events(&events).map_err(ExecuteError::EncodeEvent)?;
        let expected_state = self
            .occ
            .resolve(current_version, self.command.default_expected_state());
        let append_outcome = self
            .event_store
            .append_events(stream_id, expected_state, encoded_events)
            .await
            .map_err(ExecuteError::Append)?;

        for event in events.iter().cloned() {
            state = C::evolve(state, event).map_err(ExecuteError::Domain)?;
        }

        if self.snapshots.policy.should_snapshot(
            append_outcome.next_expected_version,
            &state,
            &events,
        ) && let Some(snapshot_payload) = C::snapshot_state(&state)
        {
            self.snapshots
                .snapshot_store
                .save_snapshot(
                    self.snapshots.snapshot_config,
                    stream_id,
                    Snapshot::new(append_outcome.next_expected_version, snapshot_payload),
                )
                .await
                .map_err(ExecuteError::SaveSnapshot)?;
        }

        Ok(ExecutionResult {
            next_expected_version: append_outcome.next_expected_version,
            events,
            state,
        })
    }
}

fn encode_events<E>(events: &NonEmpty<E>) -> serde_json::Result<NonEmpty<EventData>>
where
    E: EventType + StreamEvent + Serialize,
{
    let mut encoded_events = Vec::with_capacity(events.len());
    for event in events.iter() {
        encoded_events.push(EventData {
            event_id: uuid::Uuid::new_v4().to_string(),
            event_type: event.event_type().to_string(),
            stream_id: event.stream_id().to_string(),
            data: serde_json::to_string(event)?,
            metadata: None,
        });
    }

    NonEmpty::from_vec(encoded_events).ok_or_else(|| {
        serde_json::Error::io(std::io::Error::other(
            "failed to encode a non-empty event decision",
        ))
    })
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
        default_expected_state: Option<ExpectedState>,
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
    enum TestDomainError {
        BrokenEvent,
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
        expected_versions: Arc<Mutex<Vec<ExpectedState>>>,
        appended_events: Arc<Mutex<Vec<EventData>>>,
        saved_snapshots: Arc<Mutex<Vec<Snapshot<TestState>>>>,
    }

    impl TestCommand {
        fn new(id: &str, action: TestAction) -> Self {
            Self {
                id: id.to_string(),
                action,
                default_expected_state: None,
                stream_id_calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn with_default_expected_state(mut self, expected_state: ExpectedState) -> Self {
            self.default_expected_state = Some(expected_state);
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
    }

    impl CommandStateModel for TestCommand {
        type State = TestState;
        type Event = TestEvent;
        type DomainError = TestDomainError;

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
                TestEvent::Broken { .. } => Err(TestDomainError::BrokenEvent),
            }
        }
    }

    impl DefaultExpectedStateProvider for TestCommand {
        fn default_expected_state(&self) -> Option<ExpectedState> {
            self.default_expected_state
        }
    }

    impl SnapshotStateModel for TestCommand {
        type Snapshot = TestState;

        fn snapshot_state(state: &Self::State) -> Option<Self::Snapshot> {
            Some(*state)
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

    const TEST_SNAPSHOT_CONFIG: SnapshotStoreConfig<'static> =
        SnapshotStoreConfig::new("test.command.", None);

    impl EventStore<str> for FakeRuntime {
        type Error = &'static str;

        async fn current_stream_version(
            &self,
            _stream_id: &str,
        ) -> Result<Option<u64>, Self::Error> {
            Ok(self.current_version)
        }

        async fn read_stream_from(
            &self,
            _stream_id: &str,
            from_sequence: u64,
        ) -> Result<Vec<RecordedEvent>, Self::Error> {
            if self.fail_read_stream {
                return Err("read_stream");
            }
            self.read_from_sequences.lock().unwrap().push(from_sequence);
            Ok(self
                .recorded_events
                .iter()
                .filter(|event| event.log_position.unwrap_or(0) >= from_sequence)
                .cloned()
                .collect())
        }

        async fn append_events(
            &self,
            _stream_id: &str,
            expected_version: ExpectedState,
            events: NonEmpty<EventData>,
        ) -> Result<AppendOutcome, Self::Error> {
            if self.fail_append {
                return Err("append");
            }
            self.expected_versions
                .lock()
                .unwrap()
                .push(expected_version);
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
        type Error = &'static str;

        async fn load_snapshot(
            &self,
            _config: SnapshotStoreConfig<'static>,
            stream_id: &str,
        ) -> Result<Option<Snapshot<TestState>>, Self::Error> {
            if self.fail_load_snapshot {
                return Err("load_snapshot");
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
                return Err("save_snapshot");
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

        let result = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap();

        assert_eq!(result.next_expected_version, 1);
        assert_eq!(result.state, TestState::Present { enabled: true });
        assert_eq!(
            result.events,
            NonEmpty::one(TestEvent::Registered {
                id: "alpha".to_string(),
            })
        );
        assert_eq!(
            runtime.expected_versions.lock().unwrap().as_slice(),
            &[ExpectedState::NoStream]
        );
        assert_eq!(
            runtime.read_from_sequences.lock().unwrap().as_slice(),
            &[] as &[u64]
        );
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
                .snapshots(&runtime, TEST_SNAPSHOT_CONFIG, NoSnapshot)
                .execute(),
        )
        .unwrap();

        assert_eq!(runtime.read_from_sequences.lock().unwrap().as_slice(), &[2]);
        assert_eq!(
            runtime.expected_versions.lock().unwrap().as_slice(),
            &[ExpectedState::StreamRevision(2)]
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
                .snapshots(&runtime, TEST_SNAPSHOT_CONFIG, NoSnapshot)
                .execute(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ExecuteError::SnapshotAheadOfStream {
                snapshot_version: 3,
                stream_version: Some(2),
            }
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
                .snapshots(&runtime, TEST_SNAPSHOT_CONFIG, NoSnapshot)
                .execute(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ExecuteError::SnapshotAheadOfStream {
                snapshot_version: 1,
                stream_version: None,
            }
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

        let error = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap_err();

        assert!(matches!(
            error,
            ExecuteError::Domain(TestDomainError::BrokenEvent)
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
                .snapshots(&runtime, TEST_SNAPSHOT_CONFIG, NoSnapshot)
                .execute(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ExecuteError::Decision(TestDecisionError::AlreadyRegistered)
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
                .snapshots(&runtime, TEST_SNAPSHOT_CONFIG, NoSnapshot)
                .execute(),
        )
        .unwrap();
        let appended_events = runtime.appended_events.lock().unwrap();

        assert_eq!(result.next_expected_version, 2);
        assert_eq!(
            runtime.expected_versions.lock().unwrap().as_slice(),
            &[ExpectedState::StreamRevision(1)]
        );
        assert_eq!(appended_events.len(), 1);
        assert_eq!(appended_events[0].event_type, "state_changed");
        assert_eq!(appended_events[0].stream_id, "alpha");
        assert_eq!(result.state, TestState::Present { enabled: false });
    }

    #[test]
    fn uses_command_default_expected_state_when_requested() {
        let runtime = FakeRuntime {
            next_expected_version: 1,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register)
            .with_default_expected_state(ExpectedState::StreamExists);

        let _ = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap();

        assert_eq!(
            runtime.expected_versions.lock().unwrap().as_slice(),
            &[ExpectedState::StreamExists]
        );
    }

    #[test]
    fn explicit_occ_overrides_command_default() {
        let runtime = FakeRuntime {
            next_expected_version: 1,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register)
            .with_default_expected_state(ExpectedState::StreamExists);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .occ(OccPolicy::Explicit(ExpectedState::Any))
                .execute(),
        )
        .unwrap();

        assert_eq!(
            runtime.expected_versions.lock().unwrap().as_slice(),
            &[ExpectedState::Any]
        );
    }

    #[test]
    fn exact_current_version_overrides_command_default() {
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
            .with_default_expected_state(ExpectedState::StreamExists);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
                .occ(OccPolicy::ExactCurrentVersion)
                .execute(),
        )
        .unwrap();

        assert_eq!(
            runtime.expected_versions.lock().unwrap().as_slice(),
            &[ExpectedState::StreamRevision(7)]
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
                .snapshots(&runtime, TEST_SNAPSHOT_CONFIG, AlwaysSnapshot)
                .execute(),
        )
        .unwrap();

        assert_eq!(result.state, TestState::Present { enabled: true });
        assert_eq!(
            runtime.saved_snapshots.lock().unwrap().as_slice(),
            &[Snapshot::new(1, TestState::Present { enabled: true })]
        );
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
                .snapshots(&runtime, TEST_SNAPSHOT_CONFIG, NoSnapshot)
                .execute(),
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
                .snapshots(&runtime, TEST_SNAPSHOT_CONFIG, AlwaysSnapshot)
                .execute(),
        )
        .unwrap_err();

        assert!(matches!(error, ExecuteError::SaveSnapshot("save_snapshot")));
    }

    #[test]
    fn resolves_stream_id_once() {
        let runtime = FakeRuntime {
            next_expected_version: 1,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap();

        assert_eq!(command.stream_id_calls(), 1);
        assert!(runtime.loaded_stream_ids.lock().unwrap().is_empty());
    }
}
