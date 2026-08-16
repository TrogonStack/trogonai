use std::{
    convert::Infallible,
    sync::{Arc, Mutex},
};

use async_nats::jetstream::{self, kv};
use serde::{Deserialize, Serialize};
use trogon_decider_runtime::{
    CommandError, CommandExecution, Decider, Decision, EventData, EventDecode, EventDecodeOutcome, EventEncode,
    EventIdentity, EventType, ReadFrom, ReadStreamRequest, ReplayLimit, ReplayLimitExceeded, StreamRead,
    StreamWritePrecondition, WritePrecondition,
};
use trogon_nats::test_support::JetStreamTestServer;

use crate::{
    JetStreamStore, JetStreamStoreError, OptimisticConcurrencyConflictError, StreamStoreError, StreamSubject,
    StreamSubjectResolver, SubjectState, subject_current_position,
};

const CREATED_EVENT_TYPE: &str = "test.command-execution.created.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreateState {
    Missing,
    Created,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CreatedEvent {
    stream_id: String,
}

impl EventIdentity for CreatedEvent {}

impl EventType for CreatedEvent {
    type Error = Infallible;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        Ok(CREATED_EVENT_TYPE)
    }
}

impl EventEncode for CreatedEvent {
    type Error = serde_json::Error;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(self)
    }
}

impl EventDecode for CreatedEvent {
    type Error = serde_json::Error;

    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        if event.event_type != CREATED_EVENT_TYPE {
            return Ok(EventDecodeOutcome::Skipped);
        }

        serde_json::from_slice(event.payload).map(EventDecodeOutcome::Decoded)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("stream already exists")]
struct AlreadyExistsError;

#[derive(Debug, Clone)]
struct CreateCommand {
    stream_id: String,
    observed_states: Arc<Mutex<Vec<CreateState>>>,
}

impl CreateCommand {
    fn new(stream_id: &str) -> Self {
        Self {
            stream_id: stream_id.to_string(),
            observed_states: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn observed_states(&self) -> Vec<CreateState> {
        self.observed_states.lock().expect("lock observed states").clone()
    }
}

impl Decider for CreateCommand {
    type StreamId = str;
    type State = CreateState;
    type Event = CreatedEvent;
    type DecideError = AlreadyExistsError;
    type EvolveError = Infallible;
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::NoStream;

    fn stream_id(&self) -> &Self::StreamId {
        &self.stream_id
    }

    fn initial_state() -> Self::State {
        CreateState::Missing
    }

    fn evolve(_state: Self::State, _event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        Ok(CreateState::Created)
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        command
            .observed_states
            .lock()
            .expect("lock observed states")
            .push(*state);

        match state {
            CreateState::Missing => Ok(Decision::event(CreatedEvent {
                stream_id: command.stream_id.clone(),
            })),
            CreateState::Created => Err(AlreadyExistsError),
        }
    }
}

const APPENDED_EVENT_TYPE: &str = "test.command-execution.appended.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AppendedEvent {
    stream_id: String,
}

impl EventIdentity for AppendedEvent {}

impl EventType for AppendedEvent {
    type Error = Infallible;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        Ok(APPENDED_EVENT_TYPE)
    }
}

impl EventEncode for AppendedEvent {
    type Error = serde_json::Error;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(self)
    }
}

impl EventDecode for AppendedEvent {
    type Error = serde_json::Error;

    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        if event.event_type != APPENDED_EVENT_TYPE {
            return Ok(EventDecodeOutcome::Skipped);
        }

        serde_json::from_slice(event.payload).map(EventDecodeOutcome::Decoded)
    }
}

/// A command that unconditionally appends one event, regardless of the
/// stream's current state. Unlike [`CreateCommand`], repeated calls against
/// the same stream never fail at the decide step, which is what lets these
/// tests drive a stream through several appends to exercise
/// [`StreamWritePrecondition::StreamExists`] and [`StreamWritePrecondition::At`].
#[derive(Debug, Clone)]
struct AppendCommand {
    stream_id: String,
}

impl AppendCommand {
    fn new(stream_id: &str) -> Self {
        Self {
            stream_id: stream_id.to_string(),
        }
    }
}

impl Decider for AppendCommand {
    type StreamId = str;
    type State = u32;
    type Event = AppendedEvent;
    type DecideError = Infallible;
    type EvolveError = Infallible;
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &Self::StreamId {
        &self.stream_id
    }

    fn initial_state() -> Self::State {
        0
    }

    fn evolve(state: Self::State, _event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        Ok(state + 1)
    }

    fn decide(_state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        Ok(Decision::event(AppendedEvent {
            stream_id: command.stream_id.clone(),
        }))
    }
}

/// [`AppendCommand`]'s sibling declaring [`WritePrecondition::StreamExists`].
///
/// The guard under test has to come from the decider now that a caller cannot supply one, so each
/// variant this file exercises needs its own declaration.
#[derive(Debug, Clone)]
struct ExistingAppendCommand {
    stream_id: String,
}

impl ExistingAppendCommand {
    fn new(stream_id: &str) -> Self {
        Self {
            stream_id: stream_id.to_string(),
        }
    }
}

impl Decider for ExistingAppendCommand {
    type StreamId = str;
    type State = u32;
    type Event = AppendedEvent;
    type DecideError = Infallible;
    type EvolveError = Infallible;
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamExists;

    fn stream_id(&self) -> &Self::StreamId {
        &self.stream_id
    }

    fn initial_state() -> Self::State {
        0
    }

    fn evolve(state: Self::State, _event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        Ok(state + 1)
    }

    fn decide(_state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        Ok(Decision::event(AppendedEvent {
            stream_id: command.stream_id.clone(),
        }))
    }
}

/// [`AppendCommand`]'s sibling declaring [`WritePrecondition::Any`], used to seed a stream without
/// its own guard getting in the way of the guard the test is actually about.
#[derive(Debug, Clone)]
struct UnguardedAppendCommand {
    stream_id: String,
}

impl UnguardedAppendCommand {
    fn new(stream_id: &str) -> Self {
        Self {
            stream_id: stream_id.to_string(),
        }
    }
}

impl Decider for UnguardedAppendCommand {
    type StreamId = str;
    type State = u32;
    type Event = AppendedEvent;
    type DecideError = Infallible;
    type EvolveError = Infallible;
    #[cfg_attr(
        dylint_lib = "trogon_lints",
        allow(
            weakened_write_precondition,
            reason = "the fixture exists to prove an unguarded append reaches the store as one, which is the case it cannot make with a guard"
        )
    )]
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::Any;

    fn stream_id(&self) -> &Self::StreamId {
        &self.stream_id
    }

    fn initial_state() -> Self::State {
        0
    }

    fn evolve(state: Self::State, _event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        Ok(state + 1)
    }

    fn decide(_state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        Ok(Decision::event(AppendedEvent {
            stream_id: command.stream_id.clone(),
        }))
    }
}

#[derive(Debug, thiserror::Error)]
enum TestSubjectResolverError {
    #[error(transparent)]
    InvalidSubject(#[from] async_nats::SubjectError),
    #[error(transparent)]
    ReadPosition(#[from] StreamStoreError),
}

#[derive(Debug, Clone, Copy)]
struct TestSubjectResolver;

impl StreamSubjectResolver<str> for TestSubjectResolver {
    type Error = TestSubjectResolverError;

    async fn resolve_subject_state(
        &self,
        events_stream: &jetstream::stream::Stream,
        stream_id: &str,
    ) -> Result<SubjectState, Self::Error> {
        let subject = StreamSubject::new(format!("decider.commands.{stream_id}"))?;
        let current_position = subject_current_position(events_stream, &subject).await?;

        Ok(SubjectState {
            subject,
            current_position,
        })
    }
}

struct Harness {
    _server: JetStreamTestServer,
    store: JetStreamStore<TestSubjectResolver>,
}

impl Harness {
    async fn start() -> Self {
        let server = JetStreamTestServer::start().await;
        let js = server.jetstream().await;
        let events_stream = js
            .create_stream(jetstream::stream::Config {
                name: "COMMAND_EXECUTION".to_string(),
                subjects: vec!["decider.commands.>".to_string()],
                allow_atomic_publish: true,
                ..Default::default()
            })
            .await
            .expect("create command events stream");
        let snapshot_bucket = js
            .create_key_value(kv::Config {
                bucket: "COMMAND_EXECUTION_SNAPSHOTS".to_string(),
                ..Default::default()
            })
            .await
            .expect("create command snapshot bucket");
        let store =
            JetStreamStore::builder(js, events_stream, snapshot_bucket).with_subject_resolver(TestSubjectResolver);

        Self { _server: server, store }
    }
}

#[tokio::test]
async fn a_no_stream_command_creates_a_fresh_stream_through_command_execution() {
    let harness = Harness::start().await;
    let command = CreateCommand::new("fresh");

    let result = CommandExecution::new(&harness.store, &command)
        .execute()
        .await
        .expect("fresh create should succeed");

    assert_eq!(result.state, CreateState::Created);
    assert_eq!(
        result.events.as_slice(),
        [CreatedEvent {
            stream_id: "fresh".into()
        }]
    );
    assert_eq!(command.observed_states(), [CreateState::Missing]);
}

#[tokio::test]
async fn a_no_stream_command_rejects_an_existing_stream_at_append_without_replay() {
    let harness = Harness::start().await;
    let first = CreateCommand::new("existing");
    let first_result = CommandExecution::new(&harness.store, &first)
        .execute()
        .await
        .expect("first create should succeed");
    let second = CreateCommand::new("existing");

    let error = CommandExecution::new(&harness.store, &second)
        .execute()
        .await
        .expect_err("second create should conflict");

    assert!(
        matches!(
            &error,
            CommandError::Append(JetStreamStoreError::OptimisticConcurrencyConflict(
                OptimisticConcurrencyConflictError::WithPosition {
                    stream_id,
                    expected: StreamWritePrecondition::NoStream,
                    current_position,
                }
            )) if stream_id == "existing" && *current_position == first_result.stream_position
        ),
        "expected an append OCC conflict, got {error:?}"
    );
    assert_eq!(second.observed_states(), [CreateState::Missing]);
}

#[tokio::test]
async fn concurrent_no_stream_creates_have_one_winner_and_one_append_conflict() {
    let harness = Harness::start().await;
    let first = CreateCommand::new("concurrent");
    let second = CreateCommand::new("concurrent");

    let (first_result, second_result) = tokio::join!(
        CommandExecution::new(&harness.store, &first).execute(),
        CommandExecution::new(&harness.store, &second).execute(),
    );

    let (success, conflict) = match (first_result, second_result) {
        (Ok(success), Err(conflict)) | (Err(conflict), Ok(success)) => (success, conflict),
        (first_result, second_result) => {
            panic!("expected one success and one conflict, got {first_result:?} and {second_result:?}")
        }
    };

    assert!(matches!(
        conflict,
        CommandError::Append(JetStreamStoreError::OptimisticConcurrencyConflict(
            OptimisticConcurrencyConflictError::WithPosition {
                stream_id,
                expected: StreamWritePrecondition::NoStream,
                ..
            } | OptimisticConcurrencyConflictError::NoPosition {
                stream_id,
                expected: StreamWritePrecondition::NoStream,
            }
        )) if stream_id == "concurrent"
    ));
    assert_eq!(first.observed_states(), [CreateState::Missing]);
    assert_eq!(second.observed_states(), [CreateState::Missing]);

    let replay = harness
        .store
        .read_stream(ReadStreamRequest {
            stream_id: "concurrent",
            from: ReadFrom::Beginning,
        })
        .await
        .expect("read concurrent stream");

    assert_eq!(replay.current_position, Some(success.stream_position));
    assert_eq!(replay.events.len(), 1);
}

#[tokio::test]
async fn a_stream_exists_command_rejects_a_missing_stream() {
    let harness = Harness::start().await;
    let command = ExistingAppendCommand::new("stream-exists-missing");

    let error = CommandExecution::new(&harness.store, &command)
        .execute()
        .await
        .expect_err("StreamExists should reject a stream with no prior events");

    assert!(
        matches!(
            &error,
            CommandError::Append(JetStreamStoreError::OptimisticConcurrencyConflict(
                OptimisticConcurrencyConflictError::NoPosition {
                    stream_id,
                    expected: StreamWritePrecondition::StreamExists,
                }
            )) if stream_id == "stream-exists-missing"
        ),
        "expected a StreamExists OCC conflict, got {error:?}"
    );
}

#[tokio::test]
async fn a_stream_exists_command_accepts_an_already_created_stream() {
    let harness = Harness::start().await;
    let stream_id = "stream-exists-present";
    let first = UnguardedAppendCommand::new(stream_id);
    CommandExecution::new(&harness.store, &first)
        .execute()
        .await
        .expect("first append should succeed");

    let second = ExistingAppendCommand::new(stream_id);
    let result = CommandExecution::new(&harness.store, &second)
        .execute()
        .await
        .expect("StreamExists should accept a stream with a prior event");

    assert_eq!(result.state, 2);
}

#[tokio::test]
async fn an_expected_revision_matching_the_current_position_is_accepted() {
    let harness = Harness::start().await;
    let stream_id = "at-precondition-expected";
    let first = UnguardedAppendCommand::new(stream_id);
    let first_result = CommandExecution::new(&harness.store, &first)
        .execute()
        .await
        .expect("first append should succeed");

    let second = AppendCommand::new(stream_id);
    let result = CommandExecution::new(&harness.store, &second)
        .with_expected_revision(first_result.stream_position)
        .execute()
        .await
        .expect("an expected revision matching the current position should succeed");

    assert_eq!(result.state, 2);
}

#[tokio::test]
async fn a_stale_expected_revision_is_rejected() {
    let harness = Harness::start().await;
    let stream_id = "at-precondition-stale";
    let first = UnguardedAppendCommand::new(stream_id);
    let first_result = CommandExecution::new(&harness.store, &first)
        .execute()
        .await
        .expect("first append should succeed");

    let second = AppendCommand::new(stream_id);
    CommandExecution::new(&harness.store, &second)
        .with_expected_revision(first_result.stream_position)
        .execute()
        .await
        .expect("second append should succeed");

    let third = AppendCommand::new(stream_id);
    let error = CommandExecution::new(&harness.store, &third)
        .with_expected_revision(first_result.stream_position)
        .execute()
        .await
        .expect_err("an expected revision against a stale position should conflict");

    assert!(
        matches!(
            &error,
            CommandError::Append(JetStreamStoreError::OptimisticConcurrencyConflict(
                OptimisticConcurrencyConflictError::WithPosition {
                    stream_id,
                    expected: StreamWritePrecondition::At(position),
                    ..
                }
            )) if stream_id == "at-precondition-stale" && *position == first_result.stream_position
        ),
        "expected an At OCC conflict, got {error:?}"
    );
}

#[tokio::test]
async fn append_with_replay_limit_caps_the_replay_read_below_the_stream_length() {
    let harness = Harness::start().await;
    let stream_id = "replay-limit-bounded";

    for _ in 0..5 {
        CommandExecution::new(&harness.store, &UnguardedAppendCommand::new(stream_id))
            .execute()
            .await
            .expect("seed append should succeed");
    }

    let limit = ReplayLimit::try_new(2).expect("non-zero replay limit");
    let command = UnguardedAppendCommand::new(stream_id);
    let error = CommandExecution::new(&harness.store, &command)
        .with_replay_limit(limit)
        .execute()
        .await
        .expect_err("a five-event stream should exceed a replay limit of two");

    assert!(
        matches!(
            &error,
            CommandError::ReplayLimitExceeded(ReplayLimitExceeded {
                limit: error_limit,
                replayed_event_count: 3,
            }) if *error_limit == limit
        ),
        "expected the read to stop at limit + 1 events instead of the full five-event stream, got {error:?}"
    );
}
