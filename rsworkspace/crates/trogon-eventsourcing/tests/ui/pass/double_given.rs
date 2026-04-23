use trogon_eventsourcing::{
    Decide, Decision, StateMachine, StreamCommand,
    testing::{TestCase, decider},
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
enum TestHistoryError {
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TestDecisionError {
    NotDisabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestCommand;

impl StreamCommand for TestCommand {
    type StreamId = str;

    fn stream_id(&self) -> &Self::StreamId {
        "alpha"
    }
}

impl StateMachine<TestEvent> for TestState {
    type EvolveError = TestHistoryError;

    fn initial_state() -> Self {
        Self::Missing
    }

    fn evolve(self, event: TestEvent) -> Result<Self, Self::EvolveError> {
        match (self, event) {
            (Self::Missing, TestEvent::Registered) => Ok(Self::Registered),
            (Self::Registered, TestEvent::Disabled) => Ok(Self::Disabled),
            (Self::Disabled, TestEvent::Removed) => Ok(Self::Missing),
            _ => Err(TestHistoryError::Invalid),
        }
    }
}

impl Decide for TestCommand {
    type State = TestState;
    type Event = TestEvent;
    type DecideError = TestDecisionError;

    fn decide(state: &Self::State, _command: &Self) -> Result<Decision<Self::Event>, Self::DecideError> {
        match state {
            TestState::Disabled => Ok(Decision::event(TestEvent::Removed)),
            _ => Err(TestDecisionError::NotDisabled),
        }
    }
}

fn main() {
    TestCase::new(decider::<TestCommand>())
        .given([TestEvent::Registered])
        .given([TestEvent::Disabled])
        .when(TestCommand)
        .then([TestEvent::Removed]);
}
