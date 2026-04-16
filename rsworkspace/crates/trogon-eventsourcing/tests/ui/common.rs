use trogon_eventsourcing::{CommandState, Decide, Decision, NonEmpty, StreamCommand};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestState {
    Missing,
    Present,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestEvent {
    Registered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestDomainError {
    InvalidHistory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestDecisionError {
    AlreadyRegistered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCommand;

impl StreamCommand for TestCommand {
    type StreamId = str;

    fn stream_id(&self) -> &Self::StreamId {
        "alpha"
    }
}

impl CommandState for TestCommand {
    type State = TestState;
    type Event = TestEvent;
    type DomainError = TestDomainError;

    fn initial_state() -> Self::State {
        TestState::Missing
    }

    fn evolve(state: Self::State, event: Self::Event) -> Result<Self::State, Self::DomainError> {
        match (state, event) {
            (TestState::Missing, TestEvent::Registered) => Ok(TestState::Present),
            (TestState::Present, TestEvent::Registered) => Err(TestDomainError::InvalidHistory),
        }
    }
}

impl Decide<TestState, TestEvent> for TestCommand {
    type Error = TestDecisionError;

    fn decide(state: &TestState, _command: &Self) -> Result<Decision<TestEvent>, Self::Error> {
        match state {
            TestState::Missing => Ok(Decision::Event(NonEmpty::one(TestEvent::Registered))),
            TestState::Present => Err(TestDecisionError::AlreadyRegistered),
        }
    }
}
