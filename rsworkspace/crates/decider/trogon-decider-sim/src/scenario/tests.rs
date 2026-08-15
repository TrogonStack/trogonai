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

fn domain_error(code: &str, message: &str, details: &[(&str, &str)]) -> host::DomainError {
    host::DomainError {
        code: code.to_string(),
        message: message.to_string(),
        details: details
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    }
}

#[test]
fn a_guest_domain_error_keeps_every_field_of_the_wit_record() {
    let error = GuestDomainError::from(domain_error(
        "order.already_placed",
        "order 7 was already placed",
        &[("cause", "duplicate submission")],
    ));

    assert_eq!(error.code(), "order.already_placed");
    assert_eq!(error.message(), "order 7 was already placed");
    assert_eq!(
        error.details(),
        [("cause".to_string(), "duplicate submission".to_string())]
    );
}

#[test]
fn a_guest_domain_error_renders_its_source_chain_after_the_message() {
    let error = GuestDomainError::from(domain_error(
        "order.rejected",
        "order 7 was rejected",
        &[("cause", "stock exhausted"), ("root", "warehouse offline")],
    ));

    assert_eq!(
        error.to_string(),
        "order.rejected: order 7 was rejected (cause: stock exhausted) (root: warehouse offline)"
    );
}

#[test]
fn a_guest_domain_error_without_causes_renders_code_and_message_only() {
    let error = GuestDomainError::from(domain_error("order.rejected", "order 7 was rejected", &[]));

    assert_eq!(error.to_string(), "order.rejected: order 7 was rejected");
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
fn then_trap_completes_a_step() {
    let scenario = SimScenario::new().when(command("a")).then_trap().when(command("b"));
    assert_eq!(scenario.steps.len(), 1);
    assert!(matches!(scenario.steps[0].expectation, Expectation::Trap));
}

#[test]
fn a_trap_on_the_last_step_is_accepted() {
    let mut scenario = SimScenario::new()
        .when(command("a"))
        .then_accepted()
        .when(command("b"))
        .then_trap();
    let mut steps = std::mem::take(&mut scenario.steps);
    scenario.current.flush_into(&mut steps);
    assert!(check_trap_is_final(&steps).is_ok());
}

#[test]
fn a_trap_before_the_last_step_is_rejected() {
    let mut scenario = SimScenario::new()
        .when(command("a"))
        .then_trap()
        .when(command("b"))
        .then_accepted();
    let mut steps = std::mem::take(&mut scenario.steps);
    scenario.current.flush_into(&mut steps);
    assert!(matches!(
        check_trap_is_final(&steps),
        Err(ScenarioError::TrapNotFinalStep { index: 0, remaining: 1 })
    ));
}

#[test]
fn a_scenario_with_no_steps_has_no_misplaced_trap() {
    assert!(check_trap_is_final(&[]).is_ok());
}

#[test]
fn a_trap_expectation_reports_what_it_got_instead() {
    assert!(matches!(
        check_outcome(Ok(vec![envelope("a.Type", b"x")]), Expectation::Trap),
        Err(ScenarioError::TrapGotEvents { count: 1 })
    ));
    assert!(matches!(
        check_outcome(
            Err(host::DecideError::Rejected(domain_error("rejected", "no", &[]))),
            Expectation::Trap
        ),
        Err(ScenarioError::TrapGotRejection { error }) if error.code() == "rejected"
    ));
    assert!(matches!(
        check_outcome(
            Err(host::DecideError::Faulted(domain_error("faulted", "no", &[]))),
            Expectation::Trap
        ),
        Err(ScenarioError::TrapGotFault { error }) if error.code() == "faulted"
    ));
}

#[test]
fn when_then_when_then_flushes_the_first_step_and_buffers_the_second() {
    let scenario = SimScenario::new()
        .when(command("a"))
        .then_accepted()
        .when(command("b"))
        .then_rejected();
    assert_eq!(scenario.steps.len(), 1);
    assert!(scenario.current.when.is_some());
    assert!(scenario.current.expectation.is_some());
}
