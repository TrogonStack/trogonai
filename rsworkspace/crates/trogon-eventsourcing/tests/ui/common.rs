use trogon_eventsourcing::{Decide, Decision};

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

impl Decide for TestCommand {
    type StreamId = str;
    type State = TestState;
    type Event = TestEvent;
    type DecideError = TestDecisionError;
    type EvolveError = TestDomainError;

    fn stream_id(&self) -> &Self::StreamId {
        "alpha"
    }

    fn initial_state() -> Self::State {
        TestState::Missing
    }

    fn evolve(state: Self::State, event: Self::Event) -> Result<Self::State, Self::EvolveError> {
        match (state, event) {
            (TestState::Missing, TestEvent::Registered) => Ok(TestState::Present),
            (TestState::Present, TestEvent::Registered) => Err(TestDomainError::InvalidHistory),
        }
    }

    fn decide(state: &TestState, _command: &Self) -> Result<Decision<TestEvent>, Self::DecideError> {
        match state {
            TestState::Missing => Ok(Decision::event(TestEvent::Registered)),
            TestState::Present => Err(TestDecisionError::AlreadyRegistered),
        }
    }
}
