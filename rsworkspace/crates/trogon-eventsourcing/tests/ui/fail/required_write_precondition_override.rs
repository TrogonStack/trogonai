#[path = "../common.rs"]
mod common;

use trogon_eventsourcing::{CommandExecution, Decide, Decision, StreamCommand, StreamState};

use common::{TestDecisionError, TestEvent, TestState};

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequiredCommand;

impl StreamCommand for RequiredCommand {
    type StreamId = str;

    const REQUIRED_WRITE_PRECONDITION: Option<StreamState> = Some(StreamState::NoStream);

    fn stream_id(&self) -> &Self::StreamId {
        "alpha"
    }
}

impl Decide for RequiredCommand {
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

fn main() {
    let command = RequiredCommand;

    let _ = CommandExecution::new(&(), &command).with_write_precondition(StreamState::Any);
}
