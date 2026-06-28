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

#[test]
fn invalid_snapshot_traps_instead_of_loading_empty_state() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    // A corrupt snapshot frame must surface as a trap, not silently degrade to
    // empty state (which would let a host decide on incomplete state).
    let result = instance.open_session(Some(b"not a valid snapshot frame"));
    assert!(
        result.is_err(),
        "a corrupt snapshot must trap, not silently load empty initial state"
    );
}

#[test]
fn then_error_matches_rejection() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    // Business rejections surface through the guest bridge with the decider's
    // distinct domain code, not a generic "rejected" placeholder.
    SimScenario::new()
        .given([light_turned_on_event("kitchen", 1)])
        .when(turn_on_command("kitchen"))
        .then_error("already-on")
        .run(&mut instance)
        .unwrap();
}

fn unknown_command() -> CommandEnvelope {
    CommandEnvelope {
        type_: "type.googleapis.com/trogonai.example.light.v1.DoesNotExist".to_string(),
        payload: Vec::new(),
    }
}

#[test]
fn descriptor_lists_light_command() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let descriptor = instance.descriptor().unwrap();
    assert_eq!(descriptor.name, "example.light");
    assert!(
        descriptor
            .commands
            .iter()
            .any(|spec| spec.command_type == TURN_ON_TYPE_URL)
    );
}

#[test]
fn stream_id_returns_command_target() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let stream = instance.stream_id(&turn_on_command("kitchen")).unwrap().unwrap();
    assert_eq!(stream, "kitchen");
}

#[test]
fn then_accepted_passes_for_fresh_turn_on() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    SimScenario::new()
        .when(turn_on_command("kitchen"))
        .then_accepted()
        .run(&mut instance)
        .unwrap();
}

#[test]
fn then_accepted_reports_rejection_mismatch() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .given([light_turned_on_event("kitchen", 1)])
        .when(turn_on_command("kitchen"))
        .then_accepted()
        .run(&mut instance)
        .unwrap_err();
    assert!(error.contains("expected acceptance, got rejection"), "{error}");
}

#[test]
fn then_accepted_reports_fault_mismatch() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(unknown_command())
        .then_accepted()
        .run(&mut instance)
        .unwrap_err();
    assert!(error.contains("expected acceptance, got fault"), "{error}");
}

#[test]
fn then_error_matches_fault_code() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    SimScenario::new()
        .when(unknown_command())
        .then_error("invalid-command")
        .run(&mut instance)
        .unwrap();
}

#[test]
fn then_error_reports_rejection_code_mismatch() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .given([light_turned_on_event("kitchen", 1)])
        .when(turn_on_command("kitchen"))
        .then_error("some-other-code")
        .run(&mut instance)
        .unwrap_err();
    assert!(error.contains("got rejection: already-on"), "{error}");
}

#[test]
fn then_error_reports_fault_mismatch() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(unknown_command())
        .then_error("some-other-code")
        .run(&mut instance)
        .unwrap_err();
    assert!(error.contains("got fault: invalid-command"), "{error}");
}

#[test]
fn then_error_reports_unexpected_events() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(turn_on_command("kitchen"))
        .then_error("already-on")
        .run(&mut instance)
        .unwrap_err();
    assert!(error.contains("got 1 event(s)"), "{error}");
}

#[test]
fn then_rejected_reports_event_count() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(turn_on_command("kitchen"))
        .then_rejected()
        .run(&mut instance)
        .unwrap_err();
    assert!(error.contains("expected rejection, got 1 event(s)"), "{error}");
}

#[test]
fn then_rejected_reports_fault_mismatch() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(unknown_command())
        .then_rejected()
        .run(&mut instance)
        .unwrap_err();
    assert!(error.contains("expected rejection, got fault"), "{error}");
}

#[test]
fn then_events_reports_count_mismatch() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(turn_on_command("kitchen"))
        .then_events([])
        .run(&mut instance)
        .unwrap_err();
    assert!(error.contains("expected 0 event(s), got 1"), "{error}");
}

#[test]
fn then_events_reports_payload_mismatch() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(turn_on_command("kitchen"))
        .then_events([light_turned_on_event("hallway", 1)])
        .run(&mut instance)
        .unwrap_err();
    assert!(error.contains("event 0 mismatch"), "{error}");
}

#[test]
fn then_events_reports_fault() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(unknown_command())
        .then_events([light_turned_on_event("kitchen", 1)])
        .run(&mut instance)
        .unwrap_err();
    assert!(error.contains("faulted: invalid-command"), "{error}");
}

#[test]
fn run_requires_a_command() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .then_rejected()
        .run(&mut instance)
        .unwrap_err();
    assert!(error.contains("scenario missing .when(...)"), "{error}");
}

#[test]
fn run_requires_a_then_expectation() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .when(turn_on_command("kitchen"))
        .run(&mut instance)
        .unwrap_err();
    assert!(error.contains("scenario missing .then_events(...)"), "{error}");
}

#[test]
fn then_events_reports_rejection() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    let error = SimScenario::new()
        .given([light_turned_on_event("kitchen", 1)])
        .when(turn_on_command("kitchen"))
        .then_events([light_turned_on_event("kitchen", 2)])
        .run(&mut instance)
        .unwrap_err();
    assert!(error.contains("rejected: already-on"), "{error}");
}

#[test]
fn default_scenario_matches_new() {
    let host = SimHost::load(&light_wasm()).unwrap();
    let mut instance = host.instantiate(()).unwrap();

    SimScenario::default()
        .when(turn_on_command("kitchen"))
        .then_accepted()
        .run(&mut instance)
        .unwrap();
}
