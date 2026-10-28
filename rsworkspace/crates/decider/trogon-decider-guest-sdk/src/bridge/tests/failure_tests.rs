use trogon_decider::Events;

use super::*;

#[derive(Clone, Copy)]
enum BatchEvent {
    Valid,
    InvalidTransition,
    InvalidEncoding,
    InvalidType,
}

#[derive(Debug, thiserror::Error)]
#[error("event codec failed")]
struct EventCodecError(#[source] FixtureCodecError);

impl EventEncode for BatchEvent {
    type Error = EventCodecError;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        match self {
            Self::InvalidEncoding => Err(EventCodecError(FixtureCodecError)),
            Self::Valid | Self::InvalidTransition | Self::InvalidType => Ok(vec![1]),
        }
    }
}

impl EventType for BatchEvent {
    type Error = EventCodecError;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        match self {
            Self::InvalidType => Err(EventCodecError(FixtureCodecError)),
            Self::Valid | Self::InvalidTransition | Self::InvalidEncoding => Ok("fixture.batch-event"),
        }
    }
}

struct AppendBatch {
    second: BatchEvent,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum BatchState {
    Empty,
    Applied,
}

impl Decider for AppendBatch {
    type StreamId = str;
    type State = BatchState;
    type Event = BatchEvent;
    type DecideError = std::convert::Infallible;
    type EvolveError = FixtureEvolveError;
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &str {
        "batch"
    }

    fn initial_state() -> Self::State {
        BatchState::Empty
    }

    fn evolve(_state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        match event {
            BatchEvent::InvalidTransition => Err(FixtureEvolveError),
            BatchEvent::Valid | BatchEvent::InvalidEncoding | BatchEvent::InvalidType => Ok(BatchState::Applied),
        }
    }

    fn decide(_state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        Ok(Decision::events(Events::from_first(
            BatchEvent::Valid,
            vec![command.second],
        )))
    }
}

#[test]
fn a_decision_that_cannot_evolve_is_a_fault_and_leaves_caller_state_unchanged() {
    let state = BatchState::Empty;
    let command = AppendBatch {
        second: BatchEvent::InvalidTransition,
    };

    let error = decide_command::<_, WireError, WireEvent>(&command, &state).unwrap_err();

    assert_eq!(error.kind, WireErrorKind::Faulted);
    assert_eq!(error.code, "evolve-failed");
    assert_eq!(
        error.details,
        vec![("cause.0".to_string(), FixtureEvolveError.to_string())]
    );
    assert_eq!(state, BatchState::Empty);
    assert!(matches!(
        evaluate_decision(state, &command),
        Err(trogon_decider::DecisionError::Evolve(FixtureEvolveError))
    ));
}

#[test]
fn encoding_failure_refuses_the_entire_batch_and_keeps_the_cause_chain() {
    let state = BatchState::Empty;
    let command = AppendBatch {
        second: BatchEvent::InvalidEncoding,
    };

    let error = decide_command::<_, WireError, WireEvent>(&command, &state).unwrap_err();

    assert_eq!(error.kind, WireErrorKind::Faulted);
    assert_eq!(error.code, "encode-failed");
    assert_eq!(error.message, "failed to encode event: event codec failed");
    assert_eq!(
        error.details,
        vec![
            ("cause.0".to_string(), "event codec failed".to_string()),
            ("cause.1".to_string(), FixtureCodecError.to_string()),
        ]
    );
    assert_eq!(state, BatchState::Empty);
}

#[test]
fn unresolved_event_type_refuses_the_entire_batch_and_preserves_its_cause() {
    let state = BatchState::Empty;
    let command = AppendBatch {
        second: BatchEvent::InvalidType,
    };

    let error = decide_command::<_, WireError, WireEvent>(&command, &state).unwrap_err();

    assert_eq!(error.kind, WireErrorKind::Faulted);
    assert_eq!(error.code, "encode-failed");
    assert_eq!(error.message, "failed to resolve event type: event codec failed");
    assert_eq!(
        error.details,
        vec![
            ("cause.0".to_string(), "event codec failed".to_string()),
            ("cause.1".to_string(), FixtureCodecError.to_string()),
        ]
    );
    assert_eq!(state, BatchState::Empty);
}
