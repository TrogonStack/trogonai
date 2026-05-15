use trogon_eventsourcing::{
    Decider, Decision,
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

impl Decider for TestCommand {
    type StreamId = str;
    type State = TestState;
    type Event = TestEvent;
    type DecideError = TestDecisionError;
    type EvolveError = TestHistoryError;

    fn stream_id(&self) -> &Self::StreamId {
        "alpha"
    }

    fn initial_state() -> Self::State {
        TestState::Missing
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        match (state, event) {
            (TestState::Missing, TestEvent::Registered) => Ok(TestState::Registered),
            (TestState::Registered, TestEvent::Disabled) => Ok(TestState::Disabled),
            (TestState::Disabled, TestEvent::Removed) => Ok(TestState::Missing),
            _ => Err(TestHistoryError::Invalid),
        }
    }

    fn decide(state: &Self::State, _command: &Self) -> Result<Decision<Self>, Self::DecideError> {
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
