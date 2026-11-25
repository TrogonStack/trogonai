use trogon_decider::{Decider, Decision, WritePrecondition, testing::TestCase};

#[derive(Debug, Clone, PartialEq, Eq)]
enum TestState {
    Missing,
    Registered,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TestEvent {
    Registered,
    Disabled,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum TestHistoryError {
    #[error("{self:?}")]
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum TestDecisionError {
    #[error("{self:?}")]
    NotDisabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestCommand;

impl Decider for TestCommand {
    type StreamId = str;
    type State = TestState;
    type Event = TestEvent;
    type DecideError = TestDecisionError;
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &Self::StreamId {
        "alpha"
    }

    fn decide(state: &Self::State, _command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        match state {
            TestState::Disabled => Ok(Decision::event(TestEvent::Removed)),
            _ => Err(TestDecisionError::NotDisabled),
        }
    }
}

fn main() {
    TestCase::<TestCommand>::new()
        .given([TestEvent::Registered])
        .given([TestEvent::Disabled])
        .when(TestCommand)
        .then([TestEvent::Removed]);
}

impl Default for TestState {
    fn default() -> Self {
        TestState::Missing
    }
}

impl trogon_decider::Evolve<TestEvent> for TestState {
    type Error = TestHistoryError;

    fn evolve(self, event: &TestEvent) -> Result<Self, Self::Error> {
        match (self, event) {
            (TestState::Missing, TestEvent::Registered) => Ok(TestState::Registered),
            (TestState::Registered, TestEvent::Disabled) => Ok(TestState::Disabled),
            (TestState::Disabled, TestEvent::Removed) => Ok(TestState::Missing),
            _ => Err(TestHistoryError::Invalid),
        }
    }
}
