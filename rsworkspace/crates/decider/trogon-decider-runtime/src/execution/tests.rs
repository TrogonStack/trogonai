use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::executor::block_on;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::*;
use crate::{
    AdmissionLimit, AuthorizationDenied, ConcurrencyAdmission, Decision, EventData, EventDecode, EventDecodeOutcome,
    EventEncode, EventIdentity, EventType, InvalidSnapshotTypeNameError, PrincipalClaim, PrincipalId, PrincipalKind,
    ReadSnapshotResponse, ReadStreamResponse, SnapshotType, SnapshotTypeName, StreamEvent, WriteSnapshotResponse,
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

/// Fixture for a decider whose `initial_state` is itself valid, so only the stream's
/// emptiness can tell "never created" from "created, nothing happened yet".
#[derive(Debug, Clone)]
struct ExistingOnlyCommand {
    id: String,
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
    type Error = InvalidSnapshotTypeNameError;

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
    WriteConflict,
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
    bounded_reads: Arc<Mutex<Vec<u64>>>,
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
            bounded_reads: Arc::new(Mutex::new(Vec::new())),
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

impl ExistingOnlyCommand {
    fn new(id: &str) -> Self {
        Self { id: id.to_string() }
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

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

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

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::NoStream;

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

impl CommandSnapshotPolicy for ExistingOnlyCommand {
    type SnapshotPolicy = NoSnapshot;

    const SNAPSHOT_POLICY: Self::SnapshotPolicy = NoSnapshot;
}

impl Decider for ExistingOnlyCommand {
    type StreamId = str;
    type State = TestState;
    type Event = TestEvent;
    type DecideError = TestDecisionError;
    type EvolveError = TestCommandError;

    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamExists;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }

    fn initial_state() -> Self::State {
        initial_test_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        evolve_test_state(state, event)
    }

    fn decide(_state: &TestState, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        Ok(Decision::event(TestEvent::StateChanged {
            id: command.id.clone(),
            enabled: false,
        }))
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

    async fn read_stream_bounded(
        &self,
        request: ReadStreamRequest<'_, str>,
        max_events: u64,
    ) -> Result<ReadStreamResponse, Self::Error> {
        self.bounded_reads.lock().unwrap().push(max_events);
        let mut response = self.read_stream(request).await?;
        response.events.truncate(max_events as usize);
        Ok(response)
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
            _ => return Err(TestInfraError::WriteConflict),
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

#[tokio::test]
async fn tokio_snapshot_task_scheduler_drain_does_not_wait_for_scheduled_tasks() {
    let scheduler = TokioSnapshotTaskScheduler;
    let release = Arc::new(tokio::sync::Notify::new());
    let task_release = Arc::clone(&release);
    let executed = Arc::new(AtomicBool::new(false));
    let task_executed = Arc::clone(&executed);

    scheduler.schedule(async move {
        task_release.notified().await;
        task_executed.store(true, Ordering::SeqCst);
    });

    scheduler.drain().await;

    assert!(!executed.load(Ordering::SeqCst));

    release.notify_one();
    tokio::task::yield_now().await;

    assert!(executed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn drainable_snapshot_task_scheduler_drain_resolves_immediately_when_idle() {
    let scheduler = DrainableSnapshotTaskScheduler::new();

    scheduler.drain().await;
}

#[tokio::test]
async fn drainable_snapshot_task_scheduler_drain_awaits_outstanding_task() {
    let scheduler = DrainableSnapshotTaskScheduler::new();
    let executed = Arc::new(AtomicBool::new(false));
    let task_executed = Arc::clone(&executed);

    scheduler.schedule(async move {
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        task_executed.store(true, Ordering::SeqCst);
    });

    scheduler.drain().await;

    assert!(executed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn drainable_snapshot_task_scheduler_shares_tracking_across_clones() {
    let scheduler = DrainableSnapshotTaskScheduler::new();
    let scheduling_clone = scheduler.clone();
    let executed = Arc::new(AtomicBool::new(false));
    let task_executed = Arc::clone(&executed);

    scheduling_clone.schedule(async move {
        tokio::task::yield_now().await;
        task_executed.store(true, Ordering::SeqCst);
    });

    scheduler.drain().await;

    assert!(executed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn drainable_snapshot_task_scheduler_drain_resolves_after_panicking_task() {
    let scheduler = DrainableSnapshotTaskScheduler::new();

    scheduler.schedule(async {
        panic!("snapshot write task panicked");
    });

    tokio::time::timeout(Duration::from_secs(5), scheduler.drain())
        .await
        .expect("drain must resolve after a scheduled task panics instead of completing normally");

    let executed = Arc::new(AtomicBool::new(false));
    let task_executed = Arc::clone(&executed);
    scheduler.schedule(async move {
        tokio::task::yield_now().await;
        task_executed.store(true, Ordering::SeqCst);
    });

    tokio::time::timeout(Duration::from_secs(5), scheduler.drain())
        .await
        .expect("drain must resolve for a later schedule/drain cycle after an earlier task panicked");

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
        (
            CommandError::ReplayLimitExceeded(ReplayLimitExceeded {
                limit: ReplayLimit::try_new(2).unwrap(),
                replayed_event_count: 5,
            }),
            "command replay read 5 events, exceeding the configured limit of 2".to_string(),
            false,
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
fn declared_precondition_resolves_without_an_expected_revision() {
    let observed = Some(position(7));

    assert_eq!(
        resolve_write_precondition(WritePrecondition::StreamUnchanged, None, observed),
        Ok(StreamWritePrecondition::At(position(7)))
    );
    assert_eq!(
        resolve_write_precondition(WritePrecondition::StreamUnchanged, None, None),
        Ok(StreamWritePrecondition::NoStream)
    );
    assert_eq!(
        resolve_write_precondition(WritePrecondition::NoStream, None, None),
        Ok(StreamWritePrecondition::NoStream)
    );
    assert_eq!(
        resolve_write_precondition(WritePrecondition::StreamExists, None, observed),
        Ok(StreamWritePrecondition::StreamExists)
    );
    assert_eq!(
        resolve_write_precondition(WritePrecondition::Any, None, observed),
        Ok(StreamWritePrecondition::Any)
    );
}

#[test]
fn an_expected_revision_strengthens_every_declaration_it_can_satisfy() {
    let observed = Some(position(7));
    let expected = position(4);

    for declared in [
        WritePrecondition::StreamUnchanged,
        WritePrecondition::StreamExists,
        WritePrecondition::Any,
    ] {
        assert_eq!(
            resolve_write_precondition(declared, Some(expected), observed),
            Ok(StreamWritePrecondition::At(expected)),
            "{declared:?} should defer to the caller's expected revision"
        );
    }
}

#[test]
fn an_expected_revision_conflicts_with_a_creation_command() {
    assert_eq!(
        resolve_write_precondition(WritePrecondition::NoStream, Some(position(4)), None),
        Err(PreconditionConflictError::CreateWithRevision)
    );
}

#[test]
fn an_expected_revision_the_stream_has_never_reached_is_rejected() {
    let expected = position(9);

    for declared in [
        WritePrecondition::StreamUnchanged,
        WritePrecondition::StreamExists,
        WritePrecondition::Any,
    ] {
        assert_eq!(
            resolve_write_precondition(declared, Some(expected), Some(position(7))),
            Err(PreconditionConflictError::RevisionAheadOfStream(
                RevisionAheadOfStream {
                    expected,
                    observed: Some(position(7)),
                }
            )),
            "{declared:?} should reject a revision no event was assigned"
        );
        assert_eq!(
            resolve_write_precondition(declared, Some(expected), None),
            Err(PreconditionConflictError::RevisionAheadOfStream(
                RevisionAheadOfStream {
                    expected,
                    observed: None,
                }
            )),
            "{declared:?} should reject a revision on a stream that does not exist"
        );
    }

    assert_eq!(
        resolve_write_precondition(WritePrecondition::Any, Some(position(7)), Some(position(7))),
        Ok(StreamWritePrecondition::At(position(7))),
        "a revision equal to the observed position is the ordinary unchanged case"
    );
}

#[test]
fn rejecting_a_fabricated_revision_names_the_stream_state_it_was_measured_against() {
    assert_eq!(
        RevisionAheadOfStream {
            expected: position(9),
            observed: Some(position(7)),
        }
        .to_string(),
        "caller expected revision 9 but the stream has only reached 7"
    );
    assert_eq!(
        RevisionAheadOfStream {
            expected: position(9),
            observed: None,
        }
        .to_string(),
        "caller expected revision 9 but the stream does not exist"
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
fn a_redelivered_command_appends_the_same_event_ids_as_its_first_attempt() {
    let command_id = CommandId::new(Uuid::now_v7());
    let command = TestCommand::new("alpha", TestAction::RegisterThenDisable);
    let execute_once = || {
        let runtime = FakeRuntime {
            stream_position: position(2),
            ..Default::default()
        };
        block_on(
            CommandExecution::new(&runtime, &command)
                .with_command_id(command_id)
                .execute(),
        )
        .unwrap();
        let appended = runtime.appended_events.lock().unwrap();
        appended.iter().map(|event| event.id).collect::<Vec<_>>()
    };

    let first_attempt = execute_once();
    let redelivery = execute_once();

    assert_eq!(first_attempt.len(), 2);
    assert_ne!(first_attempt[0], first_attempt[1]);
    assert_eq!(first_attempt, redelivery);
}

#[test]
fn without_a_command_id_each_attempt_appends_freshly_generated_event_ids() {
    let command = TestCommand::new("alpha", TestAction::Register);
    let execute_once = || {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        block_on(CommandExecution::new(&runtime, &command).execute()).unwrap();
        let appended = runtime.appended_events.lock().unwrap();
        appended.iter().map(|event| event.id).collect::<Vec<_>>()
    };

    assert_ne!(execute_once(), execute_once());
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
fn fail_on_snapshot_failure_policy_still_errors_on_ahead_of_stream() {
    let runtime = FakeRuntime {
        snapshot: Some(Snapshot::new(position(3), TestState::Present { enabled: true })),
        current_position: Some(position(2)),
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Remove);

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_snapshot(test_snapshots(&runtime, NoSnapshot))
            .with_snapshot_failure_policy(FailOnSnapshotFailure)
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
    assert!(runtime.written_snapshots.lock().unwrap().is_empty());
}

#[test]
fn fail_on_snapshot_failure_policy_still_errors_on_read_failure() {
    let runtime = FakeRuntime {
        fail_read_snapshot: true,
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Register);

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_snapshot(test_snapshots(&runtime, NoSnapshot))
            .with_snapshot_failure_policy(FailOnSnapshotFailure)
            .execute(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        CommandError::ReadSnapshot(TestInfraError::ReadSnapshot)
    ));
    assert!(runtime.written_snapshots.lock().unwrap().is_empty());
}

#[test]
fn discard_and_replay_recovers_from_snapshot_ahead_of_stream() {
    let runtime = FakeRuntime {
        snapshot: Some(Snapshot::new(position(3), TestState::Present { enabled: true })),
        current_position: Some(position(2)),
        stream_events: vec![stream_event(
            1,
            TestEvent::Registered {
                id: "alpha".to_string(),
            },
        )],
        stream_position: position(3),
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Remove);

    let result = block_on(
        CommandExecution::new(&runtime, &command)
            .with_snapshot(test_snapshots(&runtime, NoSnapshot))
            .with_snapshot_failure_policy(DiscardAndReplaySnapshotFailure)
            .execute(),
    )
    .unwrap();

    assert_eq!(result.state, TestState::Missing);
    assert_eq!(
        runtime.reads_from.lock().unwrap().as_slice(),
        &[ReadFrom::Position(position(4)), ReadFrom::Beginning]
    );
    assert_eq!(
        runtime.written_snapshots.lock().unwrap().as_slice(),
        &[Snapshot::new(position(3), TestState::Missing)]
    );
}

#[test]
fn discard_and_replay_recovery_rejects_when_the_full_replay_exceeds_the_limit() {
    let runtime = FakeRuntime {
        snapshot: Some(Snapshot::new(position(3), TestState::Present { enabled: true })),
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
                    enabled: true,
                },
            ),
        ],
        stream_position: position(3),
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Remove);
    let limit = ReplayLimit::try_new(1).unwrap();

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_replay_limit(limit)
            .with_snapshot(test_snapshots(&runtime, NoSnapshot))
            .with_snapshot_failure_policy(DiscardAndReplaySnapshotFailure)
            .execute(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        CommandError::ReplayLimitExceeded(ReplayLimitExceeded {
            limit: error_limit,
            replayed_event_count: 2,
        }) if error_limit == limit
    ));
    assert!(
        runtime.written_snapshots.lock().unwrap().is_empty(),
        "a rejected recovery must not overwrite the discarded snapshot"
    );
}

#[test]
fn discard_and_replay_recovery_succeeds_when_the_full_replay_is_within_the_limit() {
    let runtime = FakeRuntime {
        snapshot: Some(Snapshot::new(position(3), TestState::Present { enabled: true })),
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
    let limit = ReplayLimit::try_new(1).unwrap();

    let result = block_on(
        CommandExecution::new(&runtime, &command)
            .with_replay_limit(limit)
            .with_snapshot(test_snapshots(&runtime, NoSnapshot))
            .with_snapshot_failure_policy(DiscardAndReplaySnapshotFailure)
            .execute(),
    )
    .unwrap();

    assert_eq!(result.state, TestState::Present { enabled: false });
    assert_eq!(
        runtime.written_snapshots.lock().unwrap().as_slice(),
        &[Snapshot::new(position(2), TestState::Present { enabled: false })],
        "the recovery snapshot write must land so the bad snapshot cannot wedge later commands"
    );
}

#[test]
fn discard_and_replay_recovers_from_snapshot_read_failure() {
    let runtime = FakeRuntime {
        fail_read_snapshot: true,
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

    let result = block_on(
        CommandExecution::new(&runtime, &command)
            .with_snapshot(test_snapshots(&runtime, NoSnapshot))
            .with_snapshot_failure_policy(DiscardAndReplaySnapshotFailure)
            .execute(),
    )
    .unwrap();

    assert_eq!(result.state, TestState::Present { enabled: false });
    assert_eq!(runtime.reads_from.lock().unwrap().as_slice(), &[ReadFrom::Beginning]);
    assert_eq!(
        runtime.written_snapshots.lock().unwrap().as_slice(),
        &[Snapshot::new(position(2), TestState::Present { enabled: false })]
    );
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

    assert!(matches!(error, CommandError::Append(TestInfraError::WriteConflict)));
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
fn a_stale_expected_revision_is_rejected_even_though_the_stream_is_unchanged() {
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

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_expected_revision(position(4))
            .execute(),
    )
    .unwrap_err();

    assert!(
        matches!(error, CommandError::Append(TestInfraError::WriteConflict)),
        "unexpected error: {error:?}"
    );
    assert_eq!(
        runtime.stream_write_preconditions.lock().unwrap().as_slice(),
        &[StreamWritePrecondition::At(position(4))]
    );
}

#[test]
fn a_current_expected_revision_reproduces_the_observed_position_guard() {
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

    let _ = block_on(
        CommandExecution::new(&runtime, &command)
            .with_expected_revision(position(7))
            .execute(),
    )
    .unwrap();

    assert_eq!(
        runtime.stream_write_preconditions.lock().unwrap().as_slice(),
        &[StreamWritePrecondition::At(position(7))]
    );
}

#[test]
fn expected_revision_on_a_creation_command_conflicts_before_any_append() {
    let runtime = FakeRuntime {
        stream_position: position(1),
        ..Default::default()
    };
    let command = RequiredRegisterCommand::new("alpha");

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_expected_revision(position(4))
            .execute(),
    )
    .unwrap_err();

    assert!(
        matches!(
            error,
            CommandError::PreconditionConflict(PreconditionConflictError::CreateWithRevision)
        ),
        "unexpected error: {error:?}"
    );
    assert!(runtime.reads_from.lock().unwrap().is_empty());
    assert!(runtime.stream_write_preconditions.lock().unwrap().is_empty());
    assert!(runtime.appended_events.lock().unwrap().is_empty());
}

#[test]
fn declared_stream_exists_guards_the_append_without_pinning_a_position() {
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
    let command = ExistingOnlyCommand::new("alpha");

    let _ = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap();

    assert_eq!(
        runtime.stream_write_preconditions.lock().unwrap().as_slice(),
        &[StreamWritePrecondition::StreamExists]
    );
}

#[test]
fn declared_stream_exists_defers_to_an_expected_revision() {
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
    let command = ExistingOnlyCommand::new("alpha");

    let _ = block_on(
        CommandExecution::new(&runtime, &command)
            .with_expected_revision(position(1))
            .execute(),
    )
    .unwrap();

    assert_eq!(
        runtime.stream_write_preconditions.lock().unwrap().as_slice(),
        &[StreamWritePrecondition::At(position(1))]
    );
}

#[test]
fn expected_revision_on_a_creation_command_conflicts_before_any_snapshot_read() {
    let runtime = FakeRuntime {
        snapshot: Some(Snapshot::new(position(1), TestState::Missing)),
        current_position: Some(position(2)),
        stream_position: position(3),
        ..Default::default()
    };
    let command = RequiredRegisterCommand::new("alpha");

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_snapshot(test_snapshots(&runtime, NoSnapshot))
            .with_expected_revision(position(1))
            .execute(),
    )
    .unwrap_err();

    assert!(
        matches!(
            error,
            CommandError::PreconditionConflict(PreconditionConflictError::CreateWithRevision)
        ),
        "unexpected error: {error:?}"
    );
    assert!(runtime.reads_from.lock().unwrap().is_empty());
    assert!(runtime.loaded_stream_ids.lock().unwrap().is_empty());
    assert!(runtime.stream_write_preconditions.lock().unwrap().is_empty());
    assert!(runtime.appended_events.lock().unwrap().is_empty());
}

#[test]
fn required_command_rule_uses_required_stream_write_precondition() {
    let runtime = FakeRuntime {
        stream_position: position(1),
        ..Default::default()
    };
    let command = RequiredRegisterCommand::new("alpha");

    let _ = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap();

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

#[test]
fn replay_limit_exceeded_fails_before_folding_without_snapshot() {
    // The final event fails evolution, so getting ReplayLimitExceeded instead
    // of Evolve(BrokenEvent) proves the limit is enforced before folding.
    let runtime = FakeRuntime {
        current_position: Some(position(3)),
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
            stream_event(
                3,
                TestEvent::Broken {
                    id: "alpha".to_string(),
                },
            ),
        ],
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Register);
    let limit = ReplayLimit::try_new(2).unwrap();

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_replay_limit(limit)
            .execute(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        CommandError::ReplayLimitExceeded(ReplayLimitExceeded {
            limit: error_limit,
            replayed_event_count: 3,
        }) if error_limit == limit
    ));
    assert!(runtime.stream_write_preconditions.lock().unwrap().is_empty());
    assert!(runtime.appended_events.lock().unwrap().is_empty());
}

#[test]
fn replay_limit_exceeded_fails_before_folding_with_snapshot() {
    // The final event fails evolution, so getting ReplayLimitExceeded instead
    // of Evolve(BrokenEvent) proves the limit is enforced before folding.
    let runtime = FakeRuntime {
        snapshot: Some(Snapshot::new(position(1), TestState::Present { enabled: true })),
        current_position: Some(position(3)),
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
            stream_event(
                3,
                TestEvent::Broken {
                    id: "alpha".to_string(),
                },
            ),
        ],
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Disable);
    let limit = ReplayLimit::try_new(1).unwrap();

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_snapshot(test_snapshots(&runtime, NoSnapshot))
            .with_replay_limit(limit)
            .execute(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        CommandError::ReplayLimitExceeded(ReplayLimitExceeded {
            limit: error_limit,
            replayed_event_count: 2,
        }) if error_limit == limit
    ));
    assert!(runtime.stream_write_preconditions.lock().unwrap().is_empty());
    assert!(runtime.appended_events.lock().unwrap().is_empty());
}

#[test]
fn replay_limit_exceeded_reads_at_most_limit_plus_one_events() {
    let runtime = FakeRuntime {
        current_position: Some(position(5)),
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
            stream_event(
                3,
                TestEvent::StateChanged {
                    id: "alpha".to_string(),
                    enabled: true,
                },
            ),
            stream_event(
                4,
                TestEvent::StateChanged {
                    id: "alpha".to_string(),
                    enabled: false,
                },
            ),
            stream_event(
                5,
                TestEvent::StateChanged {
                    id: "alpha".to_string(),
                    enabled: true,
                },
            ),
        ],
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Register);
    let limit = ReplayLimit::try_new(2).unwrap();

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_replay_limit(limit)
            .execute(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        CommandError::ReplayLimitExceeded(ReplayLimitExceeded {
            limit: error_limit,
            replayed_event_count: 3,
        }) if error_limit == limit
    ));
    assert_eq!(
        runtime.bounded_reads.lock().unwrap().as_slice(),
        &[3],
        "the read must be capped at limit + 1 events instead of fetching the whole five-event stream"
    );
}

#[test]
fn replay_limit_exactly_at_boundary_succeeds() {
    let runtime = FakeRuntime {
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
                TestEvent::Removed {
                    id: "alpha".to_string(),
                },
            ),
        ],
        stream_position: position(3),
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Register);
    let limit = ReplayLimit::try_new(2).unwrap();

    let result = block_on(
        CommandExecution::new(&runtime, &command)
            .with_replay_limit(limit)
            .execute(),
    )
    .unwrap();

    assert_eq!(result.state, TestState::Present { enabled: true });
}

#[test]
fn replay_limit_defaults_to_unlimited() {
    let runtime = FakeRuntime {
        current_position: Some(position(3)),
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
            stream_event(
                3,
                TestEvent::StateChanged {
                    id: "alpha".to_string(),
                    enabled: true,
                },
            ),
        ],
        stream_position: position(4),
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Disable);

    let result = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap();

    assert_eq!(result.state, TestState::Present { enabled: false });
}

#[test]
fn admission_defaults_to_unlimited() {
    let runtime = FakeRuntime {
        stream_position: position(1),
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Register);

    for _ in 0..8 {
        block_on(CommandExecution::new(&runtime, &command).execute()).unwrap();
    }
}

#[test]
fn a_shed_command_reads_nothing_and_appends_nothing() {
    let runtime = FakeRuntime {
        stream_position: position(1),
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Register);
    let admission = ConcurrencyAdmission::new(AdmissionLimit::try_new(1).unwrap());
    let held = admission.admit().unwrap();

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_admission(&admission)
            .execute(),
    )
    .unwrap_err();

    assert!(
        matches!(error, CommandError::Overloaded(overloaded) if overloaded.limit().as_usize() == 1),
        "{error}"
    );
    assert!(
        runtime.reads_from.lock().unwrap().is_empty(),
        "a shed command must not reach the store"
    );
    assert!(runtime.appended_events.lock().unwrap().is_empty());
    drop(held);
}

#[test]
fn an_execution_releases_its_admission_slot_when_it_ends() {
    let runtime = FakeRuntime {
        stream_position: position(1),
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Register);
    let admission = ConcurrencyAdmission::new(AdmissionLimit::try_new(1).unwrap());

    for _ in 0..3 {
        block_on(
            CommandExecution::new(&runtime, &command)
                .with_admission(&admission)
                .execute(),
        )
        .unwrap();
        assert_eq!(admission.in_flight(), 0);
    }
}

#[test]
fn a_failed_execution_releases_its_admission_slot() {
    let runtime = FakeRuntime {
        fail_read_stream: true,
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Register);
    let admission = ConcurrencyAdmission::new(AdmissionLimit::try_new(1).unwrap());

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_admission(&admission)
            .execute(),
    )
    .unwrap_err();

    assert!(matches!(error, CommandError::ReadStream(_)), "{error}");
    assert_eq!(
        admission.in_flight(),
        0,
        "a slot released only on success would leak on every failure"
    );
}

#[test]
fn a_shed_command_is_recorded_as_shed_rather_than_faulted() {
    let overloaded = OverloadedError::new(AdmissionLimit::try_new(4).unwrap());
    let error: CommandError<
        std::convert::Infallible,
        std::convert::Infallible,
        std::convert::Infallible,
        std::convert::Infallible,
        std::convert::Infallible,
        std::convert::Infallible,
        std::convert::Infallible,
        std::convert::Infallible,
    > = CommandError::Overloaded(overloaded);

    assert_eq!(decision_outcome_for_error(&error), attribute::DecisionOutcome::Shed);
    assert_eq!(
        error.to_string(),
        "command shed by admission control: all 4 execution slots are in use"
    );
}

/// An authorizer that grants on one claim and counts what it was asked.
#[derive(Debug)]
struct RequireClaim {
    claim: &'static str,
    calls: AtomicUsize,
}

impl RequireClaim {
    fn new(claim: &'static str) -> Self {
        Self {
            claim,
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl CommandAuthorizer<TestCommand> for RequireClaim {
    fn authorize(&self, principal: &CommandPrincipal, _command: &TestCommand) -> Result<(), AuthorizationDenied> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if principal.has_claim(self.claim) {
            Ok(())
        } else {
            Err(AuthorizationDenied::new(format!("{} is required", self.claim)))
        }
    }
}

fn principal(id: &str, claims: &[&str]) -> CommandPrincipal {
    CommandPrincipal::new(PrincipalKind::Agent, PrincipalId::new(id).unwrap()).with_claims(
        claims
            .iter()
            .map(|claim| PrincipalClaim::new(*claim).unwrap())
            .collect(),
    )
}

#[test]
fn authorization_defaults_to_absent() {
    let runtime = FakeRuntime {
        stream_position: position(1),
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Register);

    block_on(CommandExecution::new(&runtime, &command).execute()).unwrap();
}

#[test]
fn a_denied_command_reads_nothing_and_appends_nothing() {
    let runtime = FakeRuntime {
        stream_position: position(1),
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Register);
    let authorizer = RequireClaim::new("decider.write");

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_principal(principal("agent-1", &["decider.read"]))
            .with_authorizer(&authorizer)
            .execute(),
    )
    .unwrap_err();

    assert!(
        matches!(
            error,
            CommandError::Unauthorized(UnauthorizedError::Denied(ref denied))
                if denied.reason() == "decider.write is required"
        ),
        "{error}"
    );
    assert!(
        runtime.reads_from.lock().unwrap().is_empty(),
        "a denied command must not reach the store"
    );
    assert!(runtime.appended_events.lock().unwrap().is_empty());
    assert_eq!(command.stream_id_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn an_authorized_command_runs_as_it_would_unguarded() {
    let runtime = FakeRuntime {
        stream_position: position(1),
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Register);
    let authorizer = RequireClaim::new("decider.write");

    let result = block_on(
        CommandExecution::new(&runtime, &command)
            .with_principal(principal("agent-1", &["decider.write"]))
            .with_authorizer(&authorizer)
            .execute(),
    )
    .unwrap();

    assert_eq!(result.events.len(), 1);
    assert_eq!(runtime.appended_events.lock().unwrap().len(), 1);
    assert_eq!(authorizer.calls(), 1);
}

#[test]
fn an_execution_with_an_authorizer_and_no_principal_is_denied() {
    let runtime = FakeRuntime {
        stream_position: position(1),
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Register);
    let authorizer = RequireClaim::new("decider.write");

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_authorizer(&authorizer)
            .execute(),
    )
    .unwrap_err();

    assert!(
        matches!(error, CommandError::Unauthorized(UnauthorizedError::MissingPrincipal)),
        "{error}"
    );
    assert_eq!(
        authorizer.calls(),
        0,
        "an absent principal is refused before any policy is consulted"
    );
    assert!(runtime.appended_events.lock().unwrap().is_empty());
}

#[test]
fn one_command_is_authorized_once_however_many_conflicts_it_survives() {
    let runtime = ContendedRuntime::losing(2);
    let command = TestCommand::new("alpha", TestAction::Register);
    let authorizer = RequireClaim::new("decider.write");

    block_on(
        CommandExecution::new(&runtime, &command)
            .with_principal(principal("agent-1", &["decider.write"]))
            .with_authorizer(&authorizer)
            .with_conflict_retry(retry_limit(3))
            .execute(),
    )
    .unwrap();

    assert_eq!(runtime.attempts(), 3);
    assert_eq!(
        authorizer.calls(),
        1,
        "a retry re-reads and re-decides, but it is still the one command the principal submitted"
    );
}

#[test]
fn a_denied_command_is_recorded_as_denied_rather_than_faulted() {
    let error: CommandError<
        std::convert::Infallible,
        std::convert::Infallible,
        std::convert::Infallible,
        std::convert::Infallible,
        std::convert::Infallible,
        std::convert::Infallible,
        std::convert::Infallible,
        std::convert::Infallible,
    > = CommandError::Unauthorized(UnauthorizedError::Denied(AuthorizationDenied::new(
        "decider.write is required",
    )));

    assert_eq!(decision_outcome_for_error(&error), attribute::DecisionOutcome::Denied);
    assert_eq!(
        error.to_string(),
        "command denied for this principal: decider.write is required"
    );
}

/// A store with another writer racing on the same stream.
///
/// Each append this store rejects also advances the stream, which is what makes
/// the rejection worth another round: the next read observes a position the
/// last decision could not have.
#[derive(Debug)]
struct ContendedRuntime {
    conflicts_left: Mutex<u32>,
    current_position: Mutex<Option<StreamPosition>>,
    contender_event: Option<TestEvent>,
    stream_events: Mutex<Vec<StreamEvent>>,
    reads: Mutex<Vec<ReadFrom>>,
    stream_write_preconditions: Mutex<Vec<StreamWritePrecondition>>,
    appended_events: Mutex<Vec<Event>>,
}

impl ContendedRuntime {
    /// A store that loses `conflicts` races before the append lands.
    ///
    /// The other writer leaves nothing this decider can decode, so every
    /// attempt decides from the same state and only the guard moves.
    fn losing(conflicts: u32) -> Self {
        Self {
            conflicts_left: Mutex::new(conflicts),
            current_position: Mutex::new(None),
            contender_event: None,
            stream_events: Mutex::new(Vec::new()),
            reads: Mutex::new(Vec::new()),
            stream_write_preconditions: Mutex::new(Vec::new()),
            appended_events: Mutex::new(Vec::new()),
        }
    }

    /// The same, except the other writer records an event this decider folds.
    fn writing(mut self, event: TestEvent) -> Self {
        self.contender_event = Some(event);
        self
    }

    /// The same, over a stream that already has history.
    fn starting_at(self, current_position: StreamPosition, events: Vec<StreamEvent>) -> Self {
        *self.current_position.lock().unwrap() = Some(current_position);
        *self.stream_events.lock().unwrap() = events;
        self
    }

    fn attempts(&self) -> usize {
        self.stream_write_preconditions.lock().unwrap().len()
    }
}

impl StreamRead<str> for ContendedRuntime {
    type Error = TestInfraError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        self.reads.lock().unwrap().push(request.from);
        let from_sequence = match request.from {
            ReadFrom::Beginning => 1,
            ReadFrom::Position(position) => position.as_u64(),
        };
        Ok(ReadStreamResponse {
            current_position: *self.current_position.lock().unwrap(),
            events: self
                .stream_events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event.stream_position.as_u64() >= from_sequence)
                .cloned()
                .collect(),
        })
    }
}

impl StreamAppend<str> for ContendedRuntime {
    type Error = TestInfraError;

    async fn append_stream(&self, request: AppendStreamRequest<'_, str>) -> Result<AppendStreamResponse, Self::Error> {
        self.stream_write_preconditions
            .lock()
            .unwrap()
            .push(request.stream_write_precondition);

        let mut current_position = self.current_position.lock().unwrap();
        let next = position(current_position.map_or(1, |position| position.as_u64() + 1));
        let mut conflicts_left = self.conflicts_left.lock().unwrap();
        if *conflicts_left > 0 {
            *conflicts_left -= 1;
            if let Some(event) = self.contender_event.clone() {
                self.stream_events
                    .lock()
                    .unwrap()
                    .push(stream_event(next.as_u64(), event));
            }
            *current_position = Some(next);
            return Err(TestInfraError::WriteConflict);
        }

        *current_position = Some(next);
        self.appended_events.lock().unwrap().extend(request.events);
        Ok(AppendStreamResponse { stream_position: next })
    }

    fn classify_append_failure(&self, error: &Self::Error) -> AppendFailure {
        match error {
            TestInfraError::WriteConflict => AppendFailure::WriteConflict,
            _ => AppendFailure::Fatal,
        }
    }
}

fn retry_limit(value: u32) -> ConflictRetryLimit {
    ConflictRetryLimit::try_new(value).expect("test retry limit must be non-zero")
}

#[test]
fn a_conflict_within_the_budget_is_retried_until_the_append_lands() {
    let runtime = ContendedRuntime::losing(2);
    let command = TestCommand::new("alpha", TestAction::Register);

    let result = block_on(
        CommandExecution::new(&runtime, &command)
            .with_conflict_retry(retry_limit(3))
            .execute(),
    )
    .unwrap();

    assert_eq!(result.stream_position, position(3));
    assert_eq!(
        runtime.stream_write_preconditions.lock().unwrap().as_slice(),
        &[
            StreamWritePrecondition::NoStream,
            StreamWritePrecondition::At(position(1)),
            StreamWritePrecondition::At(position(2))
        ],
        "each attempt has to guard on the position its own read observed"
    );
    assert_eq!(
        runtime.appended_events.lock().unwrap().len(),
        1,
        "only the attempt that won the race may leave events behind"
    );
    assert_eq!(
        runtime.reads.lock().unwrap().len(),
        3,
        "a retry that skipped the read would decide from state it already knows is stale"
    );
}

#[test]
fn a_retry_decides_again_from_what_the_other_writer_left() {
    let runtime = ContendedRuntime::losing(1).writing(TestEvent::Registered {
        id: "alpha".to_string(),
    });
    let command = TestCommand::new("alpha", TestAction::Register);

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_conflict_retry(retry_limit(3))
            .execute(),
    )
    .unwrap_err();

    assert!(
        matches!(error, CommandError::Decide(TestDecisionError::AlreadyRegistered)),
        "unexpected error: {error:?}"
    );
    assert!(
        runtime.appended_events.lock().unwrap().is_empty(),
        "the retry has to honour the decision it reached on the newer state"
    );
}

#[test]
fn a_conflict_that_outlives_the_budget_reaches_the_caller() {
    let runtime = ContendedRuntime::losing(5);
    let command = TestCommand::new("alpha", TestAction::Register);

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_conflict_retry(retry_limit(2))
            .execute(),
    )
    .unwrap_err();

    assert!(
        matches!(error, CommandError::Append(TestInfraError::WriteConflict)),
        "unexpected error: {error:?}"
    );
    assert_eq!(
        runtime.attempts(),
        3,
        "the limit counts retries, so two of them means three appends attempted"
    );
}

#[test]
fn without_a_configured_limit_the_first_conflict_is_the_answer() {
    let runtime = ContendedRuntime::losing(1);
    let command = TestCommand::new("alpha", TestAction::Register);

    let error = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap_err();

    assert!(
        matches!(error, CommandError::Append(TestInfraError::WriteConflict)),
        "unexpected error: {error:?}"
    );
    assert_eq!(runtime.attempts(), 1);
}

#[test]
fn a_store_that_never_classified_its_errors_is_never_retried() {
    let runtime = FakeRuntime {
        current_position: Some(position(1)),
        stream_events: vec![stream_event(
            1,
            TestEvent::Registered {
                id: "alpha".to_string(),
            },
        )],
        ..Default::default()
    };
    let command = RequiredRegisterCommand::new("alpha");

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_conflict_retry(retry_limit(3))
            .execute(),
    )
    .unwrap_err();

    assert!(
        matches!(error, CommandError::Append(TestInfraError::WriteConflict)),
        "unexpected error: {error:?}"
    );
    assert_eq!(
        runtime.stream_write_preconditions.lock().unwrap().len(),
        1,
        "a default classification means fatal, whatever the error happens to be named"
    );
}

#[test]
fn a_creation_command_is_not_retried_past_its_own_precondition() {
    let runtime = ContendedRuntime::losing(1);
    let command = RequiredRegisterCommand::new("alpha");

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_conflict_retry(retry_limit(3))
            .execute(),
    )
    .unwrap_err();

    assert!(
        matches!(error, CommandError::Append(TestInfraError::WriteConflict)),
        "unexpected error: {error:?}"
    );
    assert_eq!(
        runtime.attempts(),
        1,
        "re-reading cannot make a stream that now exists stop existing"
    );
}

#[test]
fn a_caller_supplied_revision_is_not_retried_past() {
    let runtime = ContendedRuntime::losing(1).starting_at(
        position(1),
        vec![stream_event(
            1,
            TestEvent::Registered {
                id: "alpha".to_string(),
            },
        )],
    );
    let command = TestCommand::new("alpha", TestAction::Remove);

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_conflict_retry(retry_limit(3))
            .with_expected_revision(position(1))
            .execute(),
    )
    .unwrap_err();

    assert!(
        matches!(error, CommandError::Append(TestInfraError::WriteConflict)),
        "unexpected error: {error:?}"
    );
    assert_eq!(
        runtime.attempts(),
        1,
        "the conflict is the answer the caller asked for by naming a revision"
    );
}

fn chunk_size(value: u64) -> ReplayChunkSize {
    ReplayChunkSize::try_new(value).expect("test chunk size must be non-zero")
}

fn state_changed(sequence: u64, enabled: bool) -> StreamEvent {
    stream_event(
        sequence,
        TestEvent::StateChanged {
            id: "alpha".to_string(),
            enabled,
        },
    )
}

fn registered(sequence: u64) -> StreamEvent {
    stream_event(
        sequence,
        TestEvent::Registered {
            id: "alpha".to_string(),
        },
    )
}

#[test]
fn a_replay_chunk_size_walks_the_stream_in_bounded_reads() {
    let runtime = FakeRuntime {
        current_position: Some(position(5)),
        stream_events: vec![
            registered(1),
            state_changed(2, false),
            state_changed(3, true),
            state_changed(4, false),
            state_changed(5, true),
        ],
        stream_position: position(6),
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Remove);

    let result = block_on(
        CommandExecution::new(&runtime, &command)
            .with_replay_chunk_size(chunk_size(2))
            .execute(),
    )
    .unwrap();

    assert_eq!(result.state, TestState::Missing);
    assert_eq!(
        runtime.bounded_reads.lock().unwrap().as_slice(),
        &[2, 2, 2],
        "every read has to be capped at the chunk, or the chunk bounds nothing"
    );
    assert_eq!(
        runtime.reads_from.lock().unwrap().as_slice(),
        &[
            ReadFrom::Beginning,
            ReadFrom::Position(position(3)),
            ReadFrom::Position(position(5))
        ],
        "each read resumes after the last event the previous chunk folded"
    );
}

#[test]
fn a_chunked_replay_is_pinned_to_the_tail_the_first_read_observed() {
    let runtime = FakeRuntime {
        current_position: Some(position(3)),
        stream_events: vec![
            registered(1),
            state_changed(2, false),
            state_changed(3, true),
            state_changed(4, false),
            state_changed(5, false),
        ],
        stream_position: position(6),
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Disable);

    let result = block_on(
        CommandExecution::new(&runtime, &command)
            .with_replay_chunk_size(chunk_size(2))
            .execute(),
    )
    .unwrap();

    assert_eq!(
        result.state,
        TestState::Present { enabled: false },
        "folding past the pin would have decided from a state the append is not guarded against"
    );
    assert_eq!(
        runtime.stream_write_preconditions.lock().unwrap().as_slice(),
        &[StreamWritePrecondition::At(position(3))],
        "the guard and the fold have to agree on where the stream ended"
    );
    assert_eq!(
        runtime.bounded_reads.lock().unwrap().len(),
        2,
        "reaching the pin ends the walk, however much further the stream has grown"
    );
}

#[test]
fn a_chunk_size_and_a_limit_read_by_whichever_is_tighter() {
    let runtime = FakeRuntime {
        current_position: Some(position(6)),
        stream_events: vec![
            registered(1),
            state_changed(2, false),
            state_changed(3, true),
            state_changed(4, false),
            state_changed(5, true),
            state_changed(6, false),
        ],
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Remove);
    let limit = ReplayLimit::try_new(4).unwrap();

    let error = block_on(
        CommandExecution::new(&runtime, &command)
            .with_replay_limit(limit)
            .with_replay_chunk_size(chunk_size(3))
            .execute(),
    )
    .unwrap_err();

    assert!(
        matches!(
            error,
            CommandError::ReplayLimitExceeded(ReplayLimitExceeded {
                limit: error_limit,
                replayed_event_count: 5,
            }) if error_limit == limit
        ),
        "unexpected error: {error:?}"
    );
    assert_eq!(
        runtime.bounded_reads.lock().unwrap().as_slice(),
        &[3, 2],
        "the chunk bounds the first read and the remaining allowance plus its probe bounds the second"
    );
    assert!(
        runtime.appended_events.lock().unwrap().is_empty(),
        "a stream past its limit appends nothing, chunked or not"
    );
}

#[test]
fn replay_chunking_defaults_to_a_single_read() {
    let runtime = FakeRuntime {
        current_position: Some(position(3)),
        stream_events: vec![registered(1), state_changed(2, false), state_changed(3, true)],
        stream_position: position(4),
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Remove);

    block_on(CommandExecution::new(&runtime, &command).execute()).unwrap();

    assert_eq!(runtime.reads_from.lock().unwrap().as_slice(), &[ReadFrom::Beginning]);
    assert!(
        runtime.bounded_reads.lock().unwrap().is_empty(),
        "an unbounded replay must not start paying for a capped read it never asked for"
    );
}

#[test]
fn a_chunked_replay_resumes_from_a_snapshot_and_still_walks_the_rest() {
    let runtime = FakeRuntime {
        snapshot: Some(Snapshot::new(position(2), TestState::Present { enabled: false })),
        current_position: Some(position(6)),
        stream_events: vec![
            registered(1),
            state_changed(2, false),
            state_changed(3, true),
            state_changed(4, false),
            state_changed(5, true),
            state_changed(6, false),
        ],
        stream_position: position(7),
        ..Default::default()
    };
    let command = TestCommand::new("alpha", TestAction::Remove);

    let result = block_on(
        CommandExecution::new(&runtime, &command)
            .with_snapshot(test_snapshots(&runtime, NoSnapshot))
            .with_replay_chunk_size(chunk_size(2))
            .execute(),
    )
    .unwrap();

    assert_eq!(result.state, TestState::Missing);
    assert_eq!(
        runtime.reads_from.lock().unwrap().as_slice(),
        &[ReadFrom::Position(position(3)), ReadFrom::Position(position(5))],
        "the walk starts after the snapshot and keeps chunking from there"
    );
}
