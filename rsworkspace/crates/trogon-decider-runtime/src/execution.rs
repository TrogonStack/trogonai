//! Runtime boundary for applying decider commands to event streams.
//!
//! Deciders define pure domain behavior: how to identify a stream, rebuild
//! state from events, and decide which new events a command should emit. This
//! module owns the runtime contract around that pure core: load stream history,
//! replay it into state, evaluate the command, encode the decided events, and
//! append them with the correct stream write precondition.
//!
//! Keeping this orchestration here gives storage adapters a narrow job: read
//! and append event envelopes. It also keeps command failures tied to the phase
//! that produced them, so callers can distinguish domain rejection, replay
//! failure, codec failure, and storage failure without losing the concrete
//! source error.

use crate::stream::{
    AppendStreamRequest, AppendStreamResponse, ReadFrom, ReadStreamRequest, StreamAppend, StreamPosition, StreamRead,
    StreamWritePrecondition,
};
use crate::{
    Decider, Event, EventDecode, EventEncode, EventId, EventIdentity, EventType, Events, Headers, StreamEvent,
    WritePrecondition,
};
use trogon_decider::{DecisionFailure, evaluate_decision};
use trogon_std::{NowV7, UuidV7Generator};

type CommandEventTypeError<C> = <<C as Decider>::Event as EventType>::Error;
type CommandEventPayloadEncodeError<C> = <<C as Decider>::Event as EventEncode>::Error;
type CommandEventDecodeError<C> = <<C as Decider>::Event as EventDecode>::Error;
type CommandReadStreamError<E, C> = <E as StreamRead<<C as Decider>::StreamId>>::Error;
type CommandAppendStreamError<E, C> = <E as StreamAppend<<C as Decider>::StreamId>>::Error;
type CommandExecutionResult<E, C> = CommandResult<C, CommandReadStreamError<E, C>, CommandAppendStreamError<E, C>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionResult<State, Event> {
    /// The stream high-watermark after the command append completed.
    pub stream_position: StreamPosition,
    /// Domain events emitted by the command after successful append.
    pub events: Events<Event>,
    /// State after replaying history and applying the emitted events.
    pub state: State,
}

/// Result returned by command execution.
///
/// Command execution is the first layer that knows which phase failed, so this
/// type keeps phase information here instead of forcing storage traits to wrap
/// their own errors. The operation errors stay concrete and separate to preserve
/// compiler diagnostics and avoid boxing or a lossy shared infrastructure enum.
pub type CommandResult<C, ReadStreamError, AppendStreamError> = Result<
    ExecutionResult<<C as Decider>::State, <C as Decider>::Event>,
    CommandError<
        <C as Decider>::DecideError,
        <C as Decider>::EvolveError,
        ReadStreamError,
        AppendStreamError,
        CommandEventTypeError<C>,
        CommandEventPayloadEncodeError<C>,
        CommandEventDecodeError<C>,
    >,
>;

/// Error taxonomy for a command execution attempt.
///
/// The command boundary normalizes failures by execution phase while preserving
/// the exact source error type for each phase. Domain failures come from the
/// decider, storage failures come from the concrete read/append
/// operation that failed, and codec failures stay tied to the event traits.
#[derive(Debug)]
pub enum CommandError<
    DecideError,
    EvolveError,
    ReadStreamError,
    AppendStreamError,
    EventTypeError,
    PayloadEncodeError,
    DecodeError,
> {
    /// The command could not decide because the domain rejected it.
    Decide(DecideError),
    /// The command or replay could not evolve state from an event.
    Evolve(EvolveError),
    /// Stream history loading failed.
    ReadStream(ReadStreamError),
    /// Appending the decided events failed after the command was accepted.
    Append(AppendStreamError),
    /// A decided domain event could not provide its stored event type.
    EventType(EventTypeError),
    /// A decided domain event could not encode its payload.
    EventEncode(PayloadEncodeError),
    /// A stored event could not be converted back into a domain event.
    DecodeEvent(DecodeError),
}

enum ReplayStreamError<EvolveError, DecodeError> {
    Evolve(EvolveError),
    DecodeEvent(DecodeError),
}

enum AppendDecisionError<DecideError, EvolveError, AppendStreamError, EventTypeError, PayloadEncodeError> {
    Decide(DecideError),
    Evolve(EvolveError),
    Append(AppendStreamError),
    EventType(EventTypeError),
    EventEncode(PayloadEncodeError),
}

impl<DecideError, EvolveError, ReadStreamError, AppendStreamError, EventTypeError, PayloadEncodeError, DecodeError>
    From<ReplayStreamError<EvolveError, DecodeError>>
    for CommandError<
        DecideError,
        EvolveError,
        ReadStreamError,
        AppendStreamError,
        EventTypeError,
        PayloadEncodeError,
        DecodeError,
    >
{
    fn from(error: ReplayStreamError<EvolveError, DecodeError>) -> Self {
        match error {
            ReplayStreamError::Evolve(error) => Self::Evolve(error),
            ReplayStreamError::DecodeEvent(error) => Self::DecodeEvent(error),
        }
    }
}

impl<DecideError, EvolveError, ReadStreamError, AppendStreamError, EventTypeError, PayloadEncodeError, DecodeError>
    From<AppendDecisionError<DecideError, EvolveError, AppendStreamError, EventTypeError, PayloadEncodeError>>
    for CommandError<
        DecideError,
        EvolveError,
        ReadStreamError,
        AppendStreamError,
        EventTypeError,
        PayloadEncodeError,
        DecodeError,
    >
{
    fn from(
        error: AppendDecisionError<DecideError, EvolveError, AppendStreamError, EventTypeError, PayloadEncodeError>,
    ) -> Self {
        match error {
            AppendDecisionError::Decide(error) => Self::Decide(error),
            AppendDecisionError::Evolve(error) => Self::Evolve(error),
            AppendDecisionError::Append(error) => Self::Append(error),
            AppendDecisionError::EventType(error) => Self::EventType(error),
            AppendDecisionError::EventEncode(error) => Self::EventEncode(error),
        }
    }
}

impl<DecideError, EvolveError, ReadStreamError, AppendStreamError, EventTypeError, PayloadEncodeError, DecodeError>
    std::fmt::Display
    for CommandError<
        DecideError,
        EvolveError,
        ReadStreamError,
        AppendStreamError,
        EventTypeError,
        PayloadEncodeError,
        DecodeError,
    >
where
    DecideError: std::fmt::Display,
    EvolveError: std::fmt::Display,
    ReadStreamError: std::fmt::Display,
    AppendStreamError: std::fmt::Display,
    EventTypeError: std::fmt::Display,
    PayloadEncodeError: std::fmt::Display,
    DecodeError: std::fmt::Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decide(source) => write!(f, "command decision failed: {source}"),
            Self::Evolve(source) => write!(f, "command state evolution failed: {source}"),
            Self::ReadStream(source) => write!(f, "command stream read failed: {source}"),
            Self::Append(source) => write!(f, "command stream append failed: {source}"),
            Self::EventType(source) => write!(f, "command event type failed: {source}"),
            Self::EventEncode(source) => write!(f, "command event encoding failed: {source}"),
            Self::DecodeEvent(source) => write!(f, "command event decoding failed: {source}"),
        }
    }
}

impl<DecideError, EvolveError, ReadStreamError, AppendStreamError, EventTypeError, PayloadEncodeError, DecodeError>
    std::error::Error
    for CommandError<
        DecideError,
        EvolveError,
        ReadStreamError,
        AppendStreamError,
        EventTypeError,
        PayloadEncodeError,
        DecodeError,
    >
where
    DecideError: std::error::Error + 'static,
    EvolveError: std::error::Error + 'static,
    ReadStreamError: std::error::Error + 'static,
    AppendStreamError: std::error::Error + 'static,
    EventTypeError: std::error::Error + 'static,
    PayloadEncodeError: std::error::Error + 'static,
    DecodeError: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decide(source) => Some(source),
            Self::Evolve(source) => Some(source),
            Self::ReadStream(source) => Some(source),
            Self::Append(source) => Some(source),
            Self::EventType(source) => Some(source),
            Self::EventEncode(source) => Some(source),
            Self::DecodeEvent(source) => Some(source),
        }
    }
}

/// Builder for executing one decider command against one event stream.
///
/// The builder borrows the event store and command so execution can stay
/// allocation-light and adapter-neutral. The event-id generator is part of the
/// builder type because tests and deterministic runtimes need to replace the
/// default UUIDv7 source without weakening the production default.
pub struct CommandExecution<'a, E, C, G> {
    event_store: &'a E,
    command: &'a C,
    write_precondition: Option<StreamWritePrecondition>,
    headers: Headers,
    event_id_generator: G,
}

impl<'a, E, C> CommandExecution<'a, E, C, UuidV7Generator>
where
    C: Decider,
{
    /// Creates a command execution with default headers, precondition policy,
    /// and UUIDv7 event id generation.
    pub fn new(event_store: &'a E, command: &'a C) -> Self {
        Self {
            event_store,
            command,
            write_precondition: None,
            headers: Headers::empty(),
            event_id_generator: UuidV7Generator,
        }
    }
}

impl<'a, E, C, G> CommandExecution<'a, E, C, G> {
    /// Overrides the append precondition used when the decider does not define
    /// a command-level [`WritePrecondition`].
    pub fn with_write_precondition<W>(mut self, write_precondition: W) -> Self
    where
        W: Into<Option<StreamWritePrecondition>>,
    {
        self.write_precondition = write_precondition.into();
        self
    }

    /// Adds metadata headers to every event emitted by this command.
    pub fn with_headers(mut self, headers: Headers) -> Self {
        self.headers = headers;
        self
    }

    /// Replaces event id generation while preserving the rest of the builder.
    ///
    /// Returning a new `CommandExecution` type is intentional: the generator is
    /// a generic parameter, so deterministic test generators do not need
    /// dynamic dispatch or production-only wrapper types.
    pub fn with_event_id_generator<NextG>(self, event_id_generator: NextG) -> CommandExecution<'a, E, C, NextG>
    where
        NextG: NowV7,
    {
        CommandExecution {
            event_store: self.event_store,
            command: self.command,
            write_precondition: self.write_precondition,
            headers: self.headers,
            event_id_generator,
        }
    }
}

impl<'a, E, C, G> CommandExecution<'a, E, C, G> {
    async fn append_decision(
        &self,
        current_position: Option<StreamPosition>,
        stream_id: &C::StreamId,
        state: C::State,
    ) -> Result<
        (AppendStreamResponse, Events<C::Event>, C::State),
        AppendDecisionError<
            C::DecideError,
            C::EvolveError,
            CommandAppendStreamError<E, C>,
            CommandEventTypeError<C>,
            CommandEventPayloadEncodeError<C>,
        >,
    >
    where
        C: Decider,
        C::Event: Clone + EventType + EventIdentity + EventEncode,
        C::StreamId: AsRef<str>,
        E: StreamAppend<C::StreamId>,
        G: NowV7,
        CommandEventTypeError<C>: std::error::Error + Send + Sync + 'static,
        CommandEventPayloadEncodeError<C>: std::error::Error + Send + Sync + 'static,
    {
        let (state, events) = evaluate_decision(state, self.command).map_err(|failure| match failure {
            DecisionFailure::Decide(error) => AppendDecisionError::Decide(error),
            DecisionFailure::Evolve(error) => AppendDecisionError::Evolve(error),
        })?;
        let mut encoded_events = Vec::with_capacity(events.len());
        for event in events.iter() {
            let id = event
                .event_id()
                .unwrap_or_else(|| EventId::new(self.event_id_generator.now_v7()));
            encoded_events.push(Event {
                id,
                r#type: event.event_type().map_err(AppendDecisionError::EventType)?.to_string(),
                content: event.encode().map_err(AppendDecisionError::EventEncode)?,
                headers: self.headers.clone(),
            });
        }

        let append_outcome = self
            .event_store
            .append_stream(AppendStreamRequest {
                stream_id,
                stream_write_precondition: C::WRITE_PRECONDITION
                    .map(StreamWritePrecondition::from)
                    .or(self.write_precondition)
                    .unwrap_or_else(|| current_position.into()),
                events: encoded_events,
            })
            .await
            .map_err(AppendDecisionError::Append)?;

        Ok((append_outcome, events, state))
    }
}

impl<E, C, G> CommandExecution<'_, E, C, G>
where
    C: Decider,
    C::Event: Clone + EventType + EventIdentity + EventEncode + EventDecode,
    C::StreamId: AsRef<str>,
    E: StreamRead<C::StreamId> + StreamAppend<C::StreamId>,
    G: NowV7,
    CommandEventTypeError<C>: std::error::Error + Send + Sync + 'static,
    CommandEventPayloadEncodeError<C>: std::error::Error + Send + Sync + 'static,
    CommandEventDecodeError<C>: std::error::Error + Send + Sync + 'static,
{
    /// Executes the command by replaying stream history and appending the
    /// events decided from the rebuilt state.
    ///
    /// The read starts from the beginning of the stream because this module is
    /// the stable command boundary. Callers that want caching, projections, or
    /// alternate loading policies should layer those behind the stream traits
    /// without changing the command execution contract.
    pub async fn execute(self) -> CommandExecutionResult<E, C> {
        let stream_id = self.command.stream_id();
        let stream_read = self
            .event_store
            .read_stream(ReadStreamRequest {
                stream_id,
                from: ReadFrom::Beginning,
            })
            .await
            .map_err(CommandError::ReadStream)?;
        let current_position = stream_read.current_position;
        let state = evolve_state_from_stream_events::<C>(C::initial_state(), &stream_read.events)?;
        let (append_outcome, events, state) = self.append_decision(current_position, stream_id, state).await?;

        Ok(ExecutionResult {
            stream_position: append_outcome.stream_position,
            events,
            state,
        })
    }
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

fn evolve_state_from_stream_events<C>(
    mut state: C::State,
    stream_events: &[StreamEvent],
) -> Result<C::State, ReplayStreamError<C::EvolveError, CommandEventDecodeError<C>>>
where
    C: Decider,
    C::Event: EventDecode,
    CommandEventDecodeError<C>: std::error::Error + Send + Sync + 'static,
{
    for stream_event in stream_events {
        let event = stream_event
            .decode::<C::Event>()
            .map_err(ReplayStreamError::DecodeEvent)?;
        state = C::evolve(state, &event).map_err(ReplayStreamError::Evolve)?;
    }

    Ok(state)
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
    use uuid::Uuid;

    use super::*;
    use crate::{
        Decision, EventData, EventDecode, EventEncode, EventIdentity, EventType, ReadStreamResponse, StreamEvent,
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

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestDecisionError {
        AlreadyRegistered,
        Missing,
        AlreadyDisabled,
    }

    impl std::fmt::Display for TestDecisionError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{self:?}")
        }
    }

    impl std::error::Error for TestDecisionError {}

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestCommandError {
        BrokenEvent,
        AlreadyRegistered,
        Missing,
        AlreadyDisabled,
    }

    impl std::fmt::Display for TestCommandError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{self:?}")
        }
    }

    impl std::error::Error for TestCommandError {}

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestInfraError {
        ReadStream,
        Append,
        EventType,
        EventEncode,
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

    type TestExecutionError = CommandError<
        TestDecisionError,
        TestCommandError,
        TestInfraError,
        TestInfraError,
        TestInfraError,
        TestInfraError,
        serde_json::Error,
    >;

    #[derive(Debug, Clone)]
    struct FakeRuntime {
        current_position: Option<StreamPosition>,
        stream_events: Vec<StreamEvent>,
        stream_position: StreamPosition,
        fail_read_stream: bool,
        fail_append: bool,
        reads_from: Arc<Mutex<Vec<ReadFrom>>>,
        stream_write_preconditions: Arc<Mutex<Vec<StreamWritePrecondition>>>,
        appended_events: Arc<Mutex<Vec<Event>>>,
    }

    impl Default for FakeRuntime {
        fn default() -> Self {
            Self {
                current_position: None,
                stream_events: Vec::new(),
                stream_position: position(1),
                fail_read_stream: false,
                fail_append: false,
                reads_from: Arc::new(Mutex::new(Vec::new())),
                stream_write_preconditions: Arc::new(Mutex::new(Vec::new())),
                appended_events: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct FixedUuidGenerator(Uuid);

    impl NowV7 for FixedUuidGenerator {
        fn now_v7(&self) -> Uuid {
            self.0
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
                    .execute(|_, command: &Self| {
                        Ok(Decision::event(TestEvent::StateChanged {
                            id: command.id.clone(),
                            enabled: false,
                        }))
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
                (_, TestAction::EmitUnencodable) => {
                    Ok(Decision::event(TestEvent::Unencodable { id: command.id.clone() }))
                }
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
            serde_json::to_vec(self).map_err(|_| TestInfraError::EventEncode)
        }
    }

    impl EventDecode for TestEvent {
        type Error = serde_json::Error;

        fn decode(event: EventData<'_>) -> Result<Self, Self::Error> {
            serde_json::from_slice(event.payload)
        }
    }

    impl StreamRead<str> for FakeRuntime {
        type Error = TestInfraError;

        async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
            if self.fail_read_stream {
                return Err(TestInfraError::ReadStream);
            }
            self.reads_from.lock().unwrap().push(request.from);
            assert_eq!(request.from, ReadFrom::Beginning);
            Ok(ReadStreamResponse {
                current_position: self.current_position,
                events: self.stream_events.clone(),
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
            self.appended_events.lock().unwrap().extend(request.events);
            Ok(AppendStreamResponse {
                stream_position: self.stream_position,
            })
        }
    }

    fn stream_event(sequence: u64, event: TestEvent) -> StreamEvent {
        StreamEvent {
            stream_id: "alpha".to_string(),
            event: encode_event(&event, &UuidV7Generator, &Headers::empty()),
            stream_position: position(sequence),
            recorded_at: DateTime::<Utc>::from_timestamp(1_700_000_000 + sequence as i64, 0).unwrap(),
        }
    }

    fn invalid_stream_event(sequence: u64) -> StreamEvent {
        StreamEvent {
            stream_id: "alpha".to_string(),
            event: Event {
                id: EventId::new(Uuid::from_u128(1)),
                r#type: "registered".to_string(),
                content: b"not-json".to_vec(),
                headers: Headers::empty(),
            },
            stream_position: position(sequence),
            recorded_at: DateTime::<Utc>::from_timestamp(1_700_000_000 + sequence as i64, 0).unwrap(),
        }
    }

    #[test]
    fn executes_from_initial_state_without_history() {
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
    fn applies_headers_and_event_id_generator_builder() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);
        let headers = Headers::from_entries([("trace-id", "trace-1")]).unwrap();
        let event_id = Uuid::from_u128(42);

        let _ = block_on(
            CommandExecution::new(&runtime, &command)
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
    fn command_errors_preserve_display_and_sources() {
        let decode_error = serde_json::from_slice::<TestEvent>(b"not-json").unwrap_err();
        let cases = [
            (
                TestExecutionError::Decide(TestDecisionError::Missing),
                "command decision failed: Missing",
            ),
            (
                TestExecutionError::Evolve(TestCommandError::Missing),
                "command state evolution failed: Missing",
            ),
            (
                TestExecutionError::ReadStream(TestInfraError::ReadStream),
                "command stream read failed: ReadStream",
            ),
            (
                TestExecutionError::Append(TestInfraError::Append),
                "command stream append failed: Append",
            ),
            (
                TestExecutionError::EventType(TestInfraError::EventType),
                "command event type failed: EventType",
            ),
            (
                TestExecutionError::EventEncode(TestInfraError::EventEncode),
                "command event encoding failed: EventEncode",
            ),
            (
                TestExecutionError::DecodeEvent(decode_error),
                "command event decoding failed: expected ident at line 1 column 2",
            ),
        ];

        for (error, message) in cases {
            assert_eq!(error.to_string(), message);
            assert!(std::error::Error::source(&error).is_some());
        }
    }

    #[test]
    fn test_error_helpers_cover_all_variants() {
        assert_eq!(TestDecisionError::AlreadyRegistered.to_string(), "AlreadyRegistered");
        assert_eq!(TestDecisionError::Missing.to_string(), "Missing");
        assert_eq!(TestDecisionError::AlreadyDisabled.to_string(), "AlreadyDisabled");

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
    fn propagates_decision_failures() {
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
        let command = TestCommand::new("alpha", TestAction::Register);

        let error = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap_err();

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
            current_position: Some(position(1)),
            stream_events: vec![stream_event(
                1,
                TestEvent::StateChanged {
                    id: "alpha".to_string(),
                    enabled: false,
                },
            )],
            stream_position: position(2),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Disable);

        let error = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap_err();

        assert!(matches!(
            error,
            CommandError::Decide(TestDecisionError::AlreadyDisabled)
        ));
    }

    #[test]
    fn required_register_rejects_existing_state() {
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

        assert!(matches!(
            error,
            CommandError::Decide(TestDecisionError::AlreadyRegistered)
        ));
    }

    #[test]
    fn encodes_emitted_events_and_returns_stream_position() {
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

        let result = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap();
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
    fn resolves_stream_id_once() {
        let runtime = FakeRuntime {
            stream_position: position(1),
            ..Default::default()
        };
        let command = TestCommand::new("alpha", TestAction::Register);

        let _ = block_on(CommandExecution::new(&runtime, &command).execute()).unwrap();

        assert_eq!(command.stream_id_calls(), 1);
    }
}
