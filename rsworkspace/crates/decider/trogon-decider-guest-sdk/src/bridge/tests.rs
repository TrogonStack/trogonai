use super::*;
use trogon_decider::Decision;

#[derive(Debug, Clone, PartialEq, Eq)]
enum FixtureEvent {
    Opened { id: String },
    Funded { amount: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("fixture codec error")]
struct FixtureCodecError;

impl EventEncode for FixtureEvent {
    type Error = FixtureCodecError;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        match self {
            Self::Opened { id } => Ok(id.clone().into_bytes()),
            Self::Funded { amount } => Ok(amount.to_le_bytes().to_vec()),
        }
    }
}

impl EventType for FixtureEvent {
    type Error = std::convert::Infallible;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        Ok(match self {
            Self::Opened { .. } => "fixture.opened",
            Self::Funded { .. } => "fixture.funded",
        })
    }
}

impl EventDecode for FixtureEvent {
    type Error = FixtureCodecError;

    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        match event.event_type {
            "fixture.opened" => {
                let id = String::from_utf8(event.payload.to_vec()).map_err(|_| FixtureCodecError)?;
                Ok(EventDecodeOutcome::Decoded(Self::Opened { id }))
            }
            "fixture.funded" => {
                let bytes: [u8; 4] = event.payload.try_into().map_err(|_| FixtureCodecError)?;
                Ok(EventDecodeOutcome::Decoded(Self::Funded {
                    amount: u32::from_le_bytes(bytes),
                }))
            }
            _ => Ok(EventDecodeOutcome::Skipped),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FixtureState {
    New,
    Opened,
    Funded { amount: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
enum FixtureDecideError {
    #[error("account is already open")]
    AlreadyOpen,
    #[error("account is not open")]
    NotOpen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid fixture event for current state")]
struct FixtureEvolveError;

/// Fixture command whose `decide` returns a two-step [`Decision::Act`]: the first step opens
/// an account, the second funds it while observing the state the first step's event produced.
struct OpenAndFund {
    id: String,
    amount: u32,
}

impl Decider for OpenAndFund {
    type StreamId = str;
    type State = FixtureState;
    type Event = FixtureEvent;
    type DecideError = FixtureDecideError;
    type EvolveError = FixtureEvolveError;

    fn stream_id(&self) -> &str {
        &self.id
    }

    fn initial_state() -> FixtureState {
        FixtureState::New
    }

    fn evolve(state: FixtureState, event: &FixtureEvent) -> Result<FixtureState, FixtureEvolveError> {
        match (state, event) {
            (FixtureState::New, FixtureEvent::Opened { .. }) => Ok(FixtureState::Opened),
            (FixtureState::Opened, FixtureEvent::Funded { amount }) => Ok(FixtureState::Funded { amount: *amount }),
            _ => Err(FixtureEvolveError),
        }
    }

    fn decide(_state: &FixtureState, _command: &Self) -> Result<Decision<Self>, FixtureDecideError> {
        Decision::act()
            .execute(|state: &FixtureState, command: &Self| match state {
                FixtureState::New => Ok(Decision::event(FixtureEvent::Opened { id: command.id.clone() })),
                _ => Err(FixtureDecideError::AlreadyOpen),
            })
            .execute(|state: &FixtureState, command: &Self| match state {
                FixtureState::Opened => Ok(Decision::event(FixtureEvent::Funded { amount: command.amount })),
                _ => Err(FixtureDecideError::NotOpen),
            })
            .into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WireEvent {
    type_url: String,
    payload: Vec<u8>,
}

impl From<AnyEnvelopeParts> for WireEvent {
    fn from(value: AnyEnvelopeParts) -> Self {
        Self {
            type_url: value.type_url,
            payload: value.payload,
        }
    }
}

impl AnyEnvelopeView for WireEvent {
    fn event_type(&self) -> &str {
        &self.type_url
    }

    fn event_payload(&self) -> &[u8] {
        &self.payload
    }
}

#[derive(Debug, PartialEq, Eq)]
enum WireErrorKind {
    Rejected,
    Faulted,
}

#[derive(Debug, PartialEq, Eq)]
struct WireError {
    kind: WireErrorKind,
    code: String,
}

impl DecideErrorView for WireError {
    fn rejected(parts: DomainErrorParts) -> Self {
        Self {
            kind: WireErrorKind::Rejected,
            code: parts.code,
        }
    }

    fn faulted(parts: DomainErrorParts) -> Self {
        Self {
            kind: WireErrorKind::Faulted,
            code: parts.code,
        }
    }
}

impl From<DomainErrorParts> for WireError {
    fn from(parts: DomainErrorParts) -> Self {
        Self {
            kind: WireErrorKind::Faulted,
            code: parts.code,
        }
    }
}

fn encode_native_events(events: trogon_decider::Events<FixtureEvent>) -> Vec<WireEvent> {
    events
        .into_vec()
        .into_iter()
        .map(|event| WireEvent {
            type_url: EventType::event_type(&event).expect("infallible").to_string(),
            payload: EventEncode::encode(&event).expect("fixture events always encode"),
        })
        .collect()
}

/// The bridge's `decide_command` must produce the exact same ordered event batch as the
/// native `evaluate_decision` path for an `Act`-based decision, since the WIT `decide` export
/// and the native runtime's decision evaluation are expected to be semantically identical.
#[test]
fn act_decision_matches_native_evaluation() {
    let state = FixtureState::New;
    let command = OpenAndFund {
        id: "acct-1".to_string(),
        amount: 42,
    };

    let bridge_events =
        decide_command::<OpenAndFund, WireError, WireEvent>(&command, &state).expect("act decision should succeed");

    let (native_state, native_events) =
        evaluate_decision::<OpenAndFund>(state, &command).expect("act decision should succeed");
    let expected_events = encode_native_events(native_events);

    assert_eq!(bridge_events, expected_events);
    assert_eq!(native_state, FixtureState::Funded { amount: 42 });

    // Folding the bridge's event batch through `evolve_one`, exactly as the host does after
    // `decide` returns, must reach the same final state `evaluate_decision` computed natively.
    let mut folded_state = FixtureState::New;
    for event in bridge_events {
        folded_state = evolve_one::<OpenAndFund, WireError, WireEvent>(folded_state, event)
            .expect("bridge-encoded events must replay cleanly");
    }
    assert_eq!(folded_state, native_state);
}

/// A rejection raised by an `Act` chain's first step, when invoked against a state that step
/// does not accept, must surface through the bridge with the same stable error code the
/// native path would classify it with.
#[test]
fn act_rejection_matches_native_evaluation() {
    // An account that is already `Opened` fails the first step's `FixtureState::New` guard,
    // proving the bridge's step-level rejection and error code match native evaluation.
    let state = FixtureState::Opened;
    let command = OpenAndFund {
        id: "acct-1".to_string(),
        amount: 42,
    };

    let bridge_error =
        decide_command::<OpenAndFund, WireError, WireEvent>(&command, &state).expect_err("first step should reject");
    assert_eq!(bridge_error.kind, WireErrorKind::Rejected);
    assert_eq!(bridge_error.code, "rejected");

    let native_failure =
        evaluate_decision::<OpenAndFund>(state, &command).expect_err("first step should reject natively");
    match native_failure {
        trogon_decider::DecisionError::Decide(error) => {
            assert_eq!(error, FixtureDecideError::AlreadyOpen);
        }
        trogon_decider::DecisionError::Evolve(_) => panic!("expected a decide rejection, not an evolve failure"),
    }
}
