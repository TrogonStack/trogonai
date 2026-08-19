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
