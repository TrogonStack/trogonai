use trogon_eventsourcing::{Decide, Decision, NonEmpty, StateMachine, StreamCommand};

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

impl StateMachine<TestEvent> for TestState {
    type EvolveError = TestDomainError;

    fn initial_state() -> Self {
        TestState::Missing
    }

    fn evolve(self, event: TestEvent) -> Result<Self, Self::EvolveError> {
        match (self, event) {
            (TestState::Missing, TestEvent::Registered) => Ok(TestState::Present),
            (TestState::Present, TestEvent::Registered) => Err(TestDomainError::InvalidHistory),
        }
    }
}

impl Decide for TestCommand {
    type State = TestState;
    type Event = TestEvent;
    type DecideError = TestDecisionError;

    fn decide(state: &TestState, _command: &Self) -> Result<Decision<TestEvent>, Self::DecideError> {
        match state {
            TestState::Missing => Ok(Decision::event(TestEvent::Registered)),
            TestState::Present => Err(TestDecisionError::AlreadyRegistered),
        }
    }
}
