use std::{
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_nats::jetstream::{self, kv};
use serde::{Deserialize, Serialize};
use testcontainers_modules::nats::{Nats, NatsServerCmd};
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_decider_runtime::{
    CommandError, CommandExecution, Decider, Decision, EventData, EventDecode, EventDecodeOutcome, EventEncode,
    EventIdentity, EventType, ReadFrom, ReadStreamRequest, StreamRead, StreamWritePrecondition,
};

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
struct AlreadyExists;

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
    type DecideError = AlreadyExists;
    type EvolveError = Infallible;

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
            CreateState::Created => Err(AlreadyExists),
        }
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

struct NatsServer {
    _container: ContainerAsync<Nats>,
    url: String,
}

impl NatsServer {
    async fn start() -> Self {
        let cmd = NatsServerCmd::default().with_jetstream();
        let container = Nats::default()
            .with_cmd(&cmd)
            .start()
            .await
            .expect("start NATS testcontainer for command execution tests");
        let host = container.get_host().await.expect("get NATS testcontainer host");
        let port = container
            .get_host_port_ipv4(4222)
            .await
            .expect("get NATS testcontainer port");

        Self {
            _container: container,
            url: format!("{host}:{port}"),
        }
    }
}

struct Harness {
    _server: NatsServer,
    store: JetStreamStore<TestSubjectResolver>,
}

impl Harness {
    async fn start() -> Self {
        let server = NatsServer::start().await;
        let client = async_nats::ConnectOptions::new()
            .connection_timeout(Duration::from_secs(2))
            .connect(&server.url)
            .await
            .expect("connect to NATS testcontainer");
        let js = jetstream::new(client);
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
async fn builder_no_stream_creates_a_fresh_stream_through_command_execution() {
    let harness = Harness::start().await;
    let command = CreateCommand::new("fresh");

    let result = CommandExecution::new(&harness.store, &command)
        .with_write_precondition(StreamWritePrecondition::NoStream)
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
async fn builder_no_stream_rejects_an_existing_stream_at_append_without_replay() {
    let harness = Harness::start().await;
    let first = CreateCommand::new("existing");
    let first_result = CommandExecution::new(&harness.store, &first)
        .with_write_precondition(StreamWritePrecondition::NoStream)
        .execute()
        .await
        .expect("first create should succeed");
    let second = CreateCommand::new("existing");

    let error = CommandExecution::new(&harness.store, &second)
        .with_write_precondition(StreamWritePrecondition::NoStream)
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
async fn concurrent_builder_no_stream_creates_have_one_winner_and_one_append_conflict() {
    let harness = Harness::start().await;
    let first = CreateCommand::new("concurrent");
    let second = CreateCommand::new("concurrent");

    let (first_result, second_result) = tokio::join!(
        CommandExecution::new(&harness.store, &first)
            .with_write_precondition(StreamWritePrecondition::NoStream)
            .execute(),
        CommandExecution::new(&harness.store, &second)
            .with_write_precondition(StreamWritePrecondition::NoStream)
            .execute(),
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
