use trogon_decider::{Decider, Decision, EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType};
use trogon_decider_guest_sdk::DomainErrorParts;

use super::*;
use crate::ir::{ExpectedOutcome, ScenarioStep};

#[test]
fn native_domain_error_displays_code_and_message() {
    let error = NativeDomainError::from(DomainErrorParts {
        code: "already-exists".to_string(),
        message: "schedule already exists".to_string(),
        details: Vec::new(),
    });
    assert_eq!(error.to_string(), "already-exists: schedule already exists");
}

fn domain_error_parts() -> DomainErrorParts {
    DomainErrorParts {
        code: "rejected".to_string(),
        message: "nope".to_string(),
        details: Vec::new(),
    }
}

#[test]
fn native_decide_error_view_routes_rejected_and_faulted() {
    assert!(matches!(
        NativeDecideError::rejected(domain_error_parts()),
        NativeDecideError::Rejected(_)
    ));
    assert!(matches!(
        NativeDecideError::faulted(domain_error_parts()),
        NativeDecideError::Faulted(_)
    ));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CounterEvent {
    amount: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("counter codec error")]
struct CounterCodecError;

impl EventEncode for CounterEvent {
    type Error = CounterCodecError;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        Ok(self.amount.to_le_bytes().to_vec())
    }
}

impl EventType for CounterEvent {
    type Error = CounterCodecError;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        Ok("counter.Incremented")
    }
}

impl EventDecode for CounterEvent {
    type Error = CounterCodecError;

    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        if event.event_type != "counter.Incremented" {
            return Ok(EventDecodeOutcome::Skipped);
        }
        let bytes: [u8; 4] = event.payload.try_into().map_err(|_| CounterCodecError)?;
        Ok(EventDecodeOutcome::Decoded(CounterEvent {
            amount: u32::from_le_bytes(bytes),
        }))
    }
}

struct Increment {
    amount: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
enum CounterError {
    #[error("increment would exceed the counter ceiling")]
    TooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid counter event")]
struct CounterEvolveError;

impl Decider for Increment {
    type StreamId = str;
    type State = i64;
    type Event = CounterEvent;
    type DecideError = CounterError;
    type EvolveError = CounterEvolveError;

    fn stream_id(&self) -> &Self::StreamId {
        "counter"
    }

    fn initial_state() -> Self::State {
        0
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        Ok(state + i64::from(event.amount))
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        if state + i64::from(command.amount) > 100 {
            return Err(CounterError::TooLarge);
        }
        Ok(Decision::event(CounterEvent { amount: command.amount }))
    }
}

fn increment_envelope(amount: u32) -> WireEnvelope {
    WireEnvelope::new("counter.Increment", amount.to_le_bytes().to_vec())
}

fn incremented_envelope(amount: u32) -> WireEnvelope {
    WireEnvelope::new("counter.Incremented", amount.to_le_bytes().to_vec())
}

struct CounterBundle;

impl NativeDeciderBundle for CounterBundle {
    type State = i64;

    fn initial_state() -> Self::State {
        Increment::initial_state()
    }

    fn stream_id(_command: &WireEnvelope) -> Result<String, NativeDomainError> {
        Ok("counter".to_string())
    }

    fn evolve(state: Self::State, event: &WireEnvelope) -> Result<Self::State, NativeDomainError> {
        native_evolve_one::<Increment>(state, event)
    }

    fn decide(command: &WireEnvelope, state: &Self::State) -> Result<Vec<WireEnvelope>, NativeDecideError> {
        let bytes: [u8; 4] = command
            .payload
            .clone()
            .try_into()
            .expect("test fixture always encodes a 4-byte amount");
        let amount = u32::from_le_bytes(bytes);
        native_decide(&Increment { amount }, state)
    }
}

#[test]
fn run_native_accepts_and_folds_events_across_steps() {
    let mut scenario = ScenarioIr::new("increment twice");
    scenario.steps.push(ScenarioStep {
        when: increment_envelope(10),
        expect: ExpectedOutcome::Accepted,
    });
    scenario.steps.push(ScenarioStep {
        when: increment_envelope(20),
        expect: ExpectedOutcome::Accepted,
    });

    let run = run_native::<CounterBundle>(&scenario).expect("scenario runs");
    assert_eq!(run.steps.len(), 2);
    assert_eq!(run.steps[0], StepOutcome::Events(vec![incremented_envelope(10)]));
    assert_eq!(run.steps[1], StepOutcome::Events(vec![incremented_envelope(20)]));
}

#[test]
fn run_native_reports_rejection_once_ceiling_is_exceeded() {
    let mut scenario = ScenarioIr::new("increment past the ceiling");
    scenario.steps.push(ScenarioStep {
        when: increment_envelope(90),
        expect: ExpectedOutcome::Accepted,
    });
    scenario.steps.push(ScenarioStep {
        when: increment_envelope(90),
        expect: ExpectedOutcome::Rejected,
    });

    let run = run_native::<CounterBundle>(&scenario).expect("scenario runs");
    assert_eq!(run.steps.len(), 2);
    assert!(matches!(&run.steps[1], StepOutcome::Rejected(outcome) if outcome.code == "rejected"));
}

#[test]
fn run_native_replays_given_history_before_the_first_step() {
    let mut scenario = ScenarioIr::new("given seeds state");
    scenario.given.push(incremented_envelope(50));
    scenario.steps.push(ScenarioStep {
        when: increment_envelope(60),
        expect: ExpectedOutcome::Rejected,
    });

    let run = run_native::<CounterBundle>(&scenario).expect("scenario runs");
    assert_eq!(run.steps.len(), 1);
    assert!(matches!(&run.steps[0], StepOutcome::Rejected(_)));
}

#[test]
fn run_native_resolves_the_first_steps_stream_id() {
    let mut scenario = ScenarioIr::new("stream id resolves");
    scenario.steps.push(ScenarioStep {
        when: increment_envelope(10),
        expect: ExpectedOutcome::Accepted,
    });

    let run = run_native::<CounterBundle>(&scenario).expect("scenario runs");
    assert_eq!(run.stream_id, Some(StreamIdOutcome::Resolved("counter".to_string())));
}

#[test]
fn run_native_reports_no_stream_id_for_an_empty_scenario() {
    let scenario = ScenarioIr::new("no steps");

    let run = run_native::<CounterBundle>(&scenario).expect("scenario runs");
    assert!(run.stream_id.is_none());
}
