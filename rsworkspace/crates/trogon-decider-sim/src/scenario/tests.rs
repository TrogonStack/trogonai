use super::*;

fn envelope(type_: &str, payload: &[u8]) -> host::AnyEnvelope {
    host::AnyEnvelope {
        type_: type_.to_string(),
        payload: payload.to_vec(),
    }
}

fn command(type_: &str) -> host::CommandEnvelope {
    host::CommandEnvelope {
        type_: type_.to_string(),
        payload: Vec::new(),
    }
}

#[test]
fn events_with_different_types_never_match() {
    assert!(!events_match(&envelope("a.Type", b"x"), &envelope("b.Type", b"x")));
}

#[test]
fn unknown_types_fall_back_to_byte_comparison() {
    assert!(events_match(
        &envelope("unknown.Type", b"raw"),
        &envelope("unknown.Type", b"raw")
    ));
    assert!(!events_match(
        &envelope("unknown.Type", b"raw"),
        &envelope("unknown.Type", b"different")
    ));
}

#[test]
#[should_panic(expected = "SimScenario::when(...) called again before the previous when(...) was completed")]
fn when_twice_without_an_intervening_then_panics() {
    let _ = SimScenario::new().when(command("a")).when(command("b"));
}

#[test]
#[should_panic(expected = "SimScenario::then_*(...) called twice for the same when(...) call")]
fn then_twice_after_one_when_panics() {
    let _ = SimScenario::new().when(command("a")).then_accepted().then_rejected();
}

#[test]
#[should_panic(expected = "SimScenario::then_*(...) called without a preceding when(...) call")]
fn then_before_any_when_panics() {
    let _ = SimScenario::new().then_accepted();
}

#[test]
fn when_then_when_then_builds_two_steps_without_panicking() {
    let scenario = SimScenario::new()
        .when(command("a"))
        .then_accepted()
        .when(command("b"))
        .then_rejected();
    assert_eq!(scenario.steps.len(), 1);
    assert!(scenario.current.when.is_some());
    assert!(scenario.current.expectation.is_some());
}
