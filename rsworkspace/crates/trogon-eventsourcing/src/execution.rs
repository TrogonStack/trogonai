use serde::{Serialize, de::DeserializeOwned};

use crate::{
    Decide, Decision, EventData, EventType, NonEmpty, RecordedEvent, Snapshot, StreamCommand,
    StreamEvent,
};

pub trait CommandStateModel: StreamCommand + Sized {
    type State;
    type Event;
    type Snapshot;
    type DomainError;

    fn initial_state() -> Self::State;

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

    fn evolve(state: Self::State, event: Self::Event) -> Result<Self::State, Self::DomainError>;

    fn snapshot_state(state: &Self::State, version: u64) -> Option<Snapshot<Self::Snapshot>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedVersion {
    NoStream,
    StreamVersion(u64),
}

impl ExpectedVersion {
    pub const fn resolve(current_version: Option<u64>, override_version: Option<Self>) -> Self {
        match override_version {
            Some(expected_version) => expected_version,
            None => match current_version {
                Some(version) => Self::StreamVersion(version),
                None => Self::NoStream,
            },
        }
    }
}

pub trait ExpectedVersionProvider: StreamCommand {
    fn expected_version(&self) -> Option<ExpectedVersion> {
        None
    }
}

pub trait ExecutionRuntime<SnapshotPayload, StreamId: ?Sized>: Send + Sync {
    type Error;

    fn load_snapshot(
        &self,
        stream_id: &StreamId,
    ) -> impl std::future::Future<Output = Result<Option<Snapshot<SnapshotPayload>>, Self::Error>> + Send;

    fn save_snapshot(
        &self,
        stream_id: &StreamId,
        snapshot: Snapshot<SnapshotPayload>,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;

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
        expected_version: ExpectedVersion,
        events: NonEmpty<EventData>,
    ) -> impl std::future::Future<Output = Result<AppendOutcome, Self::Error>> + Send;
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

pub async fn execute_command<C, R, P>(
    runtime: &R,
    command: &C,
    snapshot_policy: &P,
) -> Result<ExecutionResult<C::State, C::Event>, ExecuteError<C::Error, C::DomainError, R::Error>>
where
    C: CommandStateModel + Decide<C::State, C::Event> + ExpectedVersionProvider,
    C::Event: EventType + StreamEvent + Serialize + DeserializeOwned + Clone,
    C::Snapshot: Into<C::State>,
    R: ExecutionRuntime<C::Snapshot, C::StreamId>,
    P: SnapshotPolicy<C::State, C::Event>,
{
    let stream_id = command.stream_id();
    let snapshot = runtime
        .load_snapshot(stream_id)
        .await
        .map_err(ExecuteError::LoadSnapshot)?;
    let snapshot_version = snapshot.as_ref().map(|snapshot| snapshot.version);
    let mut state = C::restore_state(command, snapshot).map_err(ExecuteError::Domain)?;
    let current_version = runtime
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
            let recorded_events = runtime
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

    let Decision::Event(events) = C::decide(&state, command).map_err(ExecuteError::Decision)?;
    let encoded_events = encode_events(&events).map_err(ExecuteError::EncodeEvent)?;
    let expected_version = ExpectedVersion::resolve(current_version, command.expected_version());
    let append_outcome = runtime
        .append_events(stream_id, expected_version, encoded_events)
        .await
        .map_err(ExecuteError::Append)?;

    for event in events.iter().cloned() {
        state = C::evolve(state, event).map_err(ExecuteError::Domain)?;
    }

    if snapshot_policy.should_snapshot(append_outcome.next_expected_version, &state, &events)
        && let Some(snapshot) = C::snapshot_state(&state, append_outcome.next_expected_version)
    {
        runtime
            .save_snapshot(stream_id, snapshot)
            .await
            .map_err(ExecuteError::SaveSnapshot)?;
    }

    Ok(ExecutionResult {
        next_expected_version: append_outcome.next_expected_version,
        events,
        state,
    })
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
        expected_versions: Arc<Mutex<Vec<ExpectedVersion>>>,
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
        type Snapshot = TestState;
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

        fn snapshot_state(state: &Self::State, version: u64) -> Option<Snapshot<Self::Snapshot>> {
            Some(Snapshot::new(version, *state))
        }
    }

    impl ExpectedVersionProvider for TestCommand {}

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

    impl ExecutionRuntime<TestState, str> for FakeRuntime {
        type Error = &'static str;

        async fn load_snapshot(
            &self,
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
            _stream_id: &str,
            snapshot: Snapshot<TestState>,
        ) -> Result<(), Self::Error> {
            if self.fail_save_snapshot {
                return Err("save_snapshot");
            }
            self.saved_snapshots.lock().unwrap().push(snapshot);
            Ok(())
        }

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
            expected_version: ExpectedVersion,
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

        let result = block_on(execute_command(&runtime, &command, &NoSnapshot)).unwrap();

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
            &[ExpectedVersion::NoStream]
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

        let result = block_on(execute_command(&runtime, &command, &NoSnapshot)).unwrap();

        assert_eq!(runtime.read_from_sequences.lock().unwrap().as_slice(), &[2]);
        assert_eq!(
            runtime.expected_versions.lock().unwrap().as_slice(),
            &[ExpectedVersion::StreamVersion(2)]
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

        let error = block_on(execute_command(&runtime, &command, &NoSnapshot)).unwrap_err();

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

        let error = block_on(execute_command(&runtime, &command, &NoSnapshot)).unwrap_err();

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

        let error = block_on(execute_command(&runtime, &command, &NoSnapshot)).unwrap_err();

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

        let error = block_on(execute_command(&runtime, &command, &NoSnapshot)).unwrap_err();

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

        let result = block_on(execute_command(&runtime, &command, &NoSnapshot)).unwrap();
        let appended_events = runtime.appended_events.lock().unwrap();

        assert_eq!(result.next_expected_version, 2);
        assert_eq!(
            runtime.expected_versions.lock().unwrap().as_slice(),
            &[ExpectedVersion::StreamVersion(1)]
        );
        assert_eq!(appended_events.len(), 1);
        assert_eq!(appended_events[0].event_type, "state_changed");
        assert_eq!(appended_events[0].stream_id, "alpha");
        assert_eq!(result.state, TestState::Present { enabled: false });
    }

    #[test]
    fn saves_snapshot_when_policy_requests_it() {
        let runtime = FakeRuntime {
            next_expected_version: 1,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let result = block_on(execute_command(&runtime, &command, &AlwaysSnapshot)).unwrap();

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

        let _ = block_on(execute_command(&runtime, &command, &NoSnapshot)).unwrap();

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

        let error = block_on(execute_command(&runtime, &command, &AlwaysSnapshot)).unwrap_err();

        assert!(matches!(error, ExecuteError::SaveSnapshot("save_snapshot")));
    }

    #[test]
    fn resolves_stream_id_once() {
        let runtime = FakeRuntime {
            next_expected_version: 1,
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(execute_command(&runtime, &command, &NoSnapshot)).unwrap();

        assert_eq!(command.stream_id_calls(), 1);
        assert_eq!(
            runtime.loaded_stream_ids.lock().unwrap().as_slice(),
            ["alpha"]
        );
    }
}
