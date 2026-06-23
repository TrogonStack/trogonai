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
    InvalidSnapshotTypeName, ReadSnapshotResponse, ReadStreamResponse, SnapshotType, SnapshotTypeName, StreamEvent,
    WriteSnapshotResponse,
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
    type Error = InvalidSnapshotTypeName;

    fn snapshot_type() -> Result<SnapshotTypeName, Self::Error> {
        SnapshotTypeName::new("test.command.v1.State")
    }
}

#[test]
fn test_state_snapshot_type_is_stable() {
    assert_eq!(TestState::snapshot_type().unwrap().as_str(), "test.command.v1.State");
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
enum TestDecisionError {
    #[error("{self:?}")]
    AlreadyRegistered,
    #[error("{self:?}")]
    Missing,
    #[error("{self:?}")]
    AlreadyDisabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
enum TestCommandError {
    #[error("{self:?}")]
    BrokenEvent,
    #[error("{self:?}")]
    AlreadyRegistered,
    #[error("{self:?}")]
    Missing,
    #[error("{self:?}")]
    AlreadyDisabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
enum TestInfraError {
    #[error("{self:?}")]
    ReadSnapshot,
    #[error("{self:?}")]
    WriteSnapshot,
    #[error("{self:?}")]
    ReadStream,
    #[error("{self:?}")]
    Append,
    #[error("{self:?}")]
    Json,
    #[error("{self:?}")]
    EventType,
    #[error("{self:?}")]
    EventEncode,
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
            (_, TestAction::EmitUnencodable) => Ok(Decision::event(TestEvent::Unencodable { id: command.id.clone() })),
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

impl CommandSnapshotPolicy for RequiredRegisterCommand {
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

    async fn append_stream(&self, request: AppendStreamRequest<'_, str>) -> Result<AppendStreamResponse, Self::Error> {
        if self.fail_append {
            return Err(TestInfraError::Append);
        }
        self.stream_write_preconditions
            .lock()
            .unwrap()
            .push(request.stream_write_precondition);
        match request.stream_write_precondition {
            StreamWritePrecondition::Any => {}
            StreamWritePrecondition::StreamExists if self.current_position.is_some() => {}
            StreamWritePrecondition::NoStream if self.current_position.is_none() => {}
            StreamWritePrecondition::At(position) if self.current_position == Some(position) => {}
            _ => return Err(TestInfraError::Append),
        }
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
            .push(request.snapshot_id.to_string());
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
fn no_stream_command_rejects_existing_stream_during_append_without_replay() {
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

    assert!(matches!(error, CommandError::Append(TestInfraError::Append)));
    assert!(runtime.reads_from.lock().unwrap().is_empty());
    assert_eq!(
        runtime.stream_write_preconditions.lock().unwrap().as_slice(),
        &[StreamWritePrecondition::NoStream]
    );
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
fn explicit_stream_exists_precondition_allows_existing_stream() {
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
    let command = TestCommand::new("alpha", TestAction::Disable);

    let _ = block_on(
        CommandExecution::new(&runtime, &command)
            .with_write_precondition(StreamWritePrecondition::StreamExists)
            .execute(),
    )
    .unwrap();

    assert_eq!(
        runtime.stream_write_preconditions.lock().unwrap().as_slice(),
        &[StreamWritePrecondition::StreamExists]
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
    assert!(runtime.reads_from.lock().unwrap().is_empty());
    assert!(runtime.loaded_stream_ids.lock().unwrap().is_empty());
}

#[test]
#[allow(
    clippy::disallowed_methods,
    reason = "unit-tests a decide guard on a fixture command; the runtime suite uses FakeRuntime, not the TestCase harness"
)]
fn required_register_decision_rejects_present_state() {
    let command = RequiredRegisterCommand::new("alpha");

    let error = RequiredRegisterCommand::decide(&TestState::Present { enabled: true }, &command).unwrap_err();

    assert_eq!(error, TestDecisionError::AlreadyRegistered);
}

#[test]
fn no_stream_command_with_snapshots_skips_snapshot_and_stream_reads() {
    let runtime = FakeRuntime {
        stream_position: position(1),
        ..Default::default()
    };
    let command = RequiredRegisterCommand::new("alpha");

    let result = block_on(
        CommandExecution::new(&runtime, &command)
            .with_snapshot(test_snapshots(&runtime, NoSnapshot))
            .execute(),
    )
    .unwrap();

    assert_eq!(result.state, TestState::Present { enabled: true });
    assert!(runtime.reads_from.lock().unwrap().is_empty());
    assert!(runtime.loaded_stream_ids.lock().unwrap().is_empty());
    assert_eq!(
        runtime.stream_write_preconditions.lock().unwrap().as_slice(),
        &[StreamWritePrecondition::NoStream]
    );
}

#[test]
fn no_stream_command_with_snapshots_writes_snapshot_when_policy_takes() {
    let runtime = FakeRuntime {
        stream_position: position(1),
        ..Default::default()
    };
    let command = RequiredRegisterCommand::new("alpha");

    let result = block_on(
        CommandExecution::new(&runtime, &command)
            .with_snapshot(test_snapshots(&runtime, FrequencySnapshot::new(NonZeroU64::MIN)))
            .execute(),
    )
    .unwrap();

    assert_eq!(result.state, TestState::Present { enabled: true });
    assert!(runtime.reads_from.lock().unwrap().is_empty());
    assert!(runtime.loaded_stream_ids.lock().unwrap().is_empty());
    assert_eq!(
        runtime.written_snapshots.lock().unwrap().as_slice(),
        &[Snapshot::new(position(1), TestState::Present { enabled: true })]
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
