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
}

impl Decide<TestState, TestEvent> for TestCommand {
    type EvolveError = TestDomainError;
    type DecideError = TestDecisionError;

    fn initial_state() -> TestState {
        TestState::Missing
    }

    fn evolve(state: TestState, event: TestEvent) -> Result<TestState, Self::EvolveError> {
        match (state, event) {
            (TestState::Missing, TestEvent::Registered) => Ok(TestState::Present),
            (TestState::Present, TestEvent::Registered) => Err(TestDomainError::InvalidHistory),
        }
    }

    fn decide(state: &TestState, _command: &Self) -> Result<Decision<TestEvent>, Self::DecideError> {
        match state {
            TestState::Missing => Ok(Decision::Event(NonEmpty::one(TestEvent::Registered))),
            TestState::Present => Err(TestDecisionError::AlreadyRegistered),
        }
    }
}
