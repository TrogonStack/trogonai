//! Integration tests against the macro-built Light decider component.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use buffa::Message as _;
use buffa::MessageName as _;
use trogon_decider_sim::{SimFixture, SimHost, SimScenario, assert_zero_imports};
use trogon_decider_wit::host::{self, CommandEnvelope};
use trogonai_proto::example::{TURN_ON_TYPE_URL, v1};

fn light_wasm() -> Vec<u8> {
    SimFixture::light().bytes().to_vec()
}

fn turn_on_command(light_id: &str) -> CommandEnvelope {
    CommandEnvelope {
        type_: TURN_ON_TYPE_URL.to_string(),
        payload: v1::TurnOn {
            light_id: light_id.to_string(),
        }
        .encode_to_vec(),
    }
}

fn light_turned_on_event(light_id: &str, count: u64) -> host::AnyEnvelope {
    host::AnyEnvelope {
        type_: v1::LightTurnedOn::FULL_NAME.to_string(),
        payload: v1::LightTurnedOn {
            light_id: light_id.to_string(),
            turn_on_count: count,
        }
        .encode_to_vec(),
    }
}

#[test]
fn light_fixture_has_no_external_imports() {
    assert_zero_imports(&light_wasm()).expect("light decider must have zero world imports");
}

#[test]
fn turn_on_from_initial_state() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    SimScenario::new()
        .when(turn_on_command("kitchen"))
        .then_events([light_turned_on_event("kitchen", 1)])
        .run(&mut instance)
        .unwrap();
}

#[test]
fn turn_on_rejects_when_already_on() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    SimScenario::new()
        .given([light_turned_on_event("kitchen", 1)])
        .when(turn_on_command("kitchen"))
        .then_rejected()
        .run(&mut instance)
        .unwrap();
}

#[test]
fn snapshot_round_trip_preserves_state() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let mut session = instance.open_session(None).unwrap();
    session.evolve(&[light_turned_on_event("kitchen", 3)]).unwrap().unwrap();
    let snapshot = session.snapshot().unwrap().expect("snapshot bytes");
    drop(session);

    let mut restored = instance.open_session(Some(&snapshot)).unwrap();
    let events = restored.decide(&turn_on_command("kitchen")).unwrap().unwrap_err();
    assert!(matches!(events, host::DecideError::Rejected(_)));
}
