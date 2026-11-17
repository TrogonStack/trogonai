use trogon_decider::{Decider, Decision, WritePrecondition};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestState {
    Missing,
    Present,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestEvent {
    Registered,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TestDomainError {
    #[error("{self:?}")]
    InvalidHistory,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TestDecisionError {
    #[error("{self:?}")]
    AlreadyRegistered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCommand;

impl Decider for TestCommand {
    type StreamId = str;
    type State = TestState;
    type Event = TestEvent;
    type DecideError = TestDecisionError;
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &Self::StreamId {
        "alpha"
    }

    fn decide(state: &TestState, _command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        match state {
            TestState::Missing => Ok(Decision::event(TestEvent::Registered)),
            TestState::Present => Err(TestDecisionError::AlreadyRegistered),
        }
    }
}

impl Default for TestState {
    fn default() -> Self {
        TestState::Missing
    }
}

impl trogon_decider::Evolve<TestEvent> for TestState {
    type Error = TestDomainError;

    fn evolve(self, event: &TestEvent) -> Result<Self, Self::Error> {
        match (self, event) {
            (TestState::Missing, TestEvent::Registered) => Ok(TestState::Present),
            (TestState::Present, TestEvent::Registered) => Err(TestDomainError::InvalidHistory),
        }
    }
}
