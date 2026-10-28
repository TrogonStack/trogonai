use std::path::PathBuf;

use super::*;

fn set(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| value.to_string()).collect()
}

fn schedules_wasm() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wasm32-unknown-unknown/release/trogon_schedules_decider.wasm");
    std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "build trogon_schedules_decider.wasm for wasm32-unknown-unknown first (expected {}): {error}",
            path.display()
        )
    })
}

fn schedules_suite() -> Suite {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schedules.yaml");
    Suite::from_yaml(&std::fs::read_to_string(path).expect("the checked-in suite is readable")).expect("it parses")
}

#[test]
fn coverage_gaps_are_counted() {
    let declared = set(&["a", "b"]);
    let exercised = set(&["a"]);
    assert_eq!(
        report_coverage_gaps(&declared, &exercised, "command", Strictness::Strict),
        1
    );
}

#[test]
fn coverage_gaps_zero_when_fully_covered() {
    let declared = set(&["a"]);
    let exercised = set(&["a"]);
    assert_eq!(
        report_coverage_gaps(&declared, &exercised, "command", Strictness::Strict),
        0
    );
}

#[test]
fn coverage_gaps_counted_regardless_of_strictness() {
    let declared = set(&["a", "b"]);
    let exercised = set(&["a"]);
    assert_eq!(
        report_coverage_gaps(&declared, &exercised, "command", Strictness::Lenient),
        1
    );
}

#[test]
fn an_output_format_is_human_or_tap() {
    assert_eq!("human".parse::<OutputFormat>().unwrap(), OutputFormat::Human);
    assert_eq!("tap".parse::<OutputFormat>().unwrap(), OutputFormat::Tap);

    let error = "xml".parse::<OutputFormat>().unwrap_err().to_string();
    assert!(error.contains("unknown format"), "unexpected error: {error}");
}

#[test]
fn the_checked_in_schedules_suite_passes() {
    run_suite(
        &schedules_wasm(),
        &schedules_suite(),
        OutputFormat::Human,
        Strictness::Strict,
    )
    .expect("schedules suite passes");
}

#[test]
fn the_checked_in_schedules_suite_passes_in_tap_format_too() {
    run_suite(
        &schedules_wasm(),
        &schedules_suite(),
        OutputFormat::Tap,
        Strictness::Strict,
    )
    .expect("schedules suite passes");
}

#[test]
fn a_suite_whose_name_does_not_match_the_component_is_refused() {
    let suite = Suite::from_yaml("suite: not.a.real.module\nscenarios: []\n").expect("it parses");

    let error = run_suite(&schedules_wasm(), &suite, OutputFormat::Human, Strictness::Strict)
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("does not match"),
        "a suite that runs green against a component it was not written for proves nothing: {error}"
    );
}

#[test]
fn failed_scenarios_accumulate_in_human_and_tap_runs() {
    let wasm = schedules_wasm();
    let mut suite = schedules_suite();
    suite.scenarios[0].then = Some(Then::Rejected { rejected: true });
    suite.scenarios[1].then = Some(Then::Rejected { rejected: false });

    for format in [OutputFormat::Human, OutputFormat::Tap] {
        let error = run_suite(&wasm, &suite, format, Strictness::Strict).unwrap_err();
        assert_eq!(error.to_string(), "2 scenario(s) failed");
    }
}

#[test]
fn strictness_controls_whether_a_suite_with_no_coverage_can_pass() {
    let wasm = schedules_wasm();
    let mut suite = schedules_suite();
    suite.scenarios.clear();

    let error = run_suite(&wasm, &suite, OutputFormat::Human, Strictness::Strict).unwrap_err();
    assert_eq!(
        error.to_string(),
        "4 declared command(s) and 4 declared event(s) have zero scenario coverage"
    );
    run_suite(&wasm, &suite, OutputFormat::Human, Strictness::Lenient)
        .expect("lenient runs report gaps without failing the suite");
}

#[test]
fn an_explicit_normal_budget_still_executes_the_scenario() {
    let wasm = schedules_wasm();
    let host = SimHost::load(&wasm).unwrap();
    let suite = schedules_suite();
    let registry = codec::type_registry(&suite.suite).unwrap();
    let mut scenario = suite.scenarios[0].to_ir(registry).unwrap();
    scenario.budget = Some(trogon_decider_sim::BudgetOverrides {
        fuel_per_call: Some(host.config().fuel_per_call()),
        ..Default::default()
    });

    run_scenario(&host, &wasm, &scenario).expect("the normal fuel budget accepts the valid create scenario");
}

#[test]
fn an_instantiation_trap_does_not_satisfy_an_accepted_expectation() {
    let wasm = schedules_wasm();
    let host = SimHost::load(&wasm).unwrap();
    let suite = schedules_suite();
    let registry = codec::type_registry(&suite.suite).unwrap();
    let mut scenario = suite
        .scenarios
        .iter()
        .find(|scenario| scenario.budget.is_some())
        .expect("the suite contains a starved-fuel scenario")
        .to_ir(registry)
        .unwrap();
    scenario.steps[0].expect = ExpectedOutcome::Accepted;

    let error = run_scenario(&host, &wasm, &scenario).unwrap_err();
    let error = error
        .downcast_ref::<trogon_decider_sim::SimError>()
        .expect("the instantiation error retains its type");
    assert!(error.is_trap());
}

#[test]
fn an_instantiation_trap_cannot_stand_in_for_multiple_steps() {
    let wasm = schedules_wasm();
    let host = SimHost::load(&wasm).unwrap();
    let suite = schedules_suite();
    let registry = codec::type_registry(&suite.suite).unwrap();
    let mut scenario = suite
        .scenarios
        .iter()
        .find(|scenario| scenario.budget.is_some())
        .expect("the suite contains a starved-fuel scenario")
        .to_ir(registry)
        .unwrap();
    scenario.steps.push(scenario.steps[0].clone());

    let error = run_scenario(&host, &wasm, &scenario).unwrap_err();
    let error = error
        .downcast_ref::<trogon_decider_sim::SimError>()
        .expect("the instantiation error retains its type");
    assert!(error.is_trap());
}

#[test]
fn an_authored_error_code_is_checked_against_the_guest_outcome() {
    let wasm = schedules_wasm();
    let mut suite = schedules_suite();
    suite.scenarios[3].then = Some(Then::Error {
        error: crate::ErrorExpectation::Structured {
            code: Some("rejected".to_string()),
            message: None,
        },
    });

    run_suite(&wasm, &suite, OutputFormat::Human, Strictness::Strict)
        .expect("pausing a missing schedule has the expected rejection code");
}
