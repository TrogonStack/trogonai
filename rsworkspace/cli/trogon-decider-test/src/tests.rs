use super::*;

fn set(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| value.to_string()).collect()
}

fn args(format: &str, no_strict: bool, wasm: PathBuf, suite: PathBuf) -> Args {
    Args {
        format: format.to_string(),
        no_strict,
        wasm,
        suite,
    }
}

fn schedules_wasm_path() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wasm32-unknown-unknown/release/trogon_schedules_decider.wasm");
    assert!(
        path.exists(),
        "build trogon_schedules_decider.wasm for wasm32-unknown-unknown first (expected {})",
        path.display()
    );
    path
}

fn schedules_suite_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schedules.yaml")
}

#[test]
fn coverage_gaps_are_counted() {
    let declared = set(&["a", "b"]);
    let exercised = set(&["a"]);
    assert_eq!(report_coverage_gaps(&declared, &exercised, "command", false), 1);
}

#[test]
fn coverage_gaps_zero_when_fully_covered() {
    let declared = set(&["a"]);
    let exercised = set(&["a"]);
    assert_eq!(report_coverage_gaps(&declared, &exercised, "command", false), 0);
}

#[test]
fn coverage_gaps_counted_regardless_of_strict_flag() {
    let declared = set(&["a", "b"]);
    let exercised = set(&["a"]);
    assert_eq!(report_coverage_gaps(&declared, &exercised, "command", true), 1);
}

#[test]
fn parse_output_format_accepts_human_and_tap() {
    assert!(matches!(parse_output_format("human").unwrap(), OutputFormat::Human));
    assert!(matches!(parse_output_format("tap").unwrap(), OutputFormat::Tap));
}

#[test]
fn parse_output_format_rejects_unknown_value() {
    let error = parse_output_format("xml").unwrap_err().to_string();
    assert!(error.contains("unknown format"), "unexpected error: {error}");
}

#[test]
fn run_passes_the_checked_in_schedules_suite() {
    run(args("human", false, schedules_wasm_path(), schedules_suite_path())).expect("schedules suite passes");
}

#[test]
fn run_passes_in_tap_format_too() {
    run(args("tap", false, schedules_wasm_path(), schedules_suite_path())).expect("schedules suite passes");
}

#[test]
fn run_rejects_a_suite_whose_name_does_not_match_the_component() {
    let suite_path = std::env::temp_dir().join("trogon-decider-test-mismatched-suite.yaml");
    fs::write(&suite_path, "suite: not.a.real.module\nscenarios: []\n").expect("write temp suite");
    let error = run(args("human", false, schedules_wasm_path(), suite_path))
        .unwrap_err()
        .to_string();
    assert!(error.contains("does not match"), "unexpected error: {error}");
}

#[test]
fn run_fails_when_the_wasm_path_does_not_exist() {
    let error = run(args(
        "human",
        false,
        PathBuf::from("/nonexistent/trogon_schedules_decider.wasm"),
        schedules_suite_path(),
    ))
    .unwrap_err()
    .to_string();
    assert!(error.contains("read"), "unexpected error: {error}");
}
