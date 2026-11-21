use std::error::Error;

use trogon_sandbox_contract::OperationState;
use trogon_sandbox_contract::OperationState::{Canceled, Expired, Failed, Queued, Running, Succeeded};

#[test]
fn every_state_preserves_its_wire_spelling_and_terminality() -> Result<(), Box<dyn Error>> {
    let fixtures = [
        (OperationState::Queued, r#""queued""#, false),
        (OperationState::Running, r#""running""#, false),
        (OperationState::Succeeded, r#""succeeded""#, true),
        (OperationState::Failed, r#""failed""#, true),
        (OperationState::Canceled, r#""canceled""#, true),
        (OperationState::Expired, r#""expired""#, true),
    ];
    assert_eq!(OperationState::ALL, fixtures.map(|(state, _, _)| state));
    for (state, json, terminal) in fixtures {
        assert_eq!(serde_json::to_string(&state)?, json);
        assert_eq!(serde_json::from_str::<OperationState>(json)?, state);
        assert_eq!(state.is_terminal(), terminal, "{state:?}");
    }
    Ok(())
}

#[test]
fn every_state_pair_preserves_the_transition_contract() {
    let states = [Queued, Running, Succeeded, Failed, Canceled, Expired];
    let transitions = [
        [false, true, false, false, true, true],
        [true, true, true, true, true, false],
        [false, false, false, false, false, false],
        [false, false, false, false, false, false],
        [false, false, false, false, false, false],
        [false, false, false, false, false, false],
    ];
    for (source, destinations) in states.into_iter().zip(transitions) {
        for (destination, allowed) in states.into_iter().zip(destinations) {
            assert_eq!(
                source.can_transition_to(destination),
                allowed,
                "{source:?} -> {destination:?}"
            );
        }
    }
}

#[test]
fn unsupported_state_spellings_are_rejected() {
    for json in [r#""Queued""#, r#""cancelled""#, r#""unknown""#, "null", "0"] {
        assert!(serde_json::from_str::<OperationState>(json).is_err(), "decoded {json}");
    }
}
