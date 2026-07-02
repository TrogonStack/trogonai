use super::*;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[test]
fn parse_output_format_accepts_known_values() {
    assert!(matches!(parse_output_format("human").unwrap(), OutputFormat::Human));
    assert!(matches!(parse_output_format("tap").unwrap(), OutputFormat::Tap));
}

#[test]
fn parse_output_format_rejects_unknown_value() {
    assert!(parse_output_format("xml").is_err());
}

#[test]
fn load_suite_parses_yaml_by_extension() {
    let suite = load_suite(&fixtures_dir().join("suite.yaml")).expect("load yaml suite");
    assert_eq!(suite.suite, "tier2-cel sample bundle");
    assert_eq!(suite.fixtures.len(), 3);
}

#[test]
fn run_test_passes_on_well_formed_suite() {
    run_test(&fixtures_dir().join("suite.yaml"), "human").expect("suite should pass");
}

#[test]
fn run_test_fails_on_deliberately_wrong_expectation() {
    let err = run_test(&fixtures_dir().join("suite_failing.yaml"), "human")
        .expect_err("suite with wrong expectation should fail");
    assert!(err.to_string().contains("fixture(s) failed"));
}

#[test]
fn run_test_tap_format_also_detects_failure() {
    let err = run_test(&fixtures_dir().join("suite_failing.yaml"), "tap").expect_err("tap format should also fail");
    assert!(err.to_string().contains("fixture(s) failed"));
}

#[test]
fn run_lint_reports_errors_on_lint_bundle_fixture() {
    let err = run_lint(&fixtures_dir().join("lint_bundle")).expect_err("lint bundle has known issues");
    assert!(err.to_string().contains("error(s)"));
}

#[test]
fn run_lint_passes_on_suite_policy_bundle() {
    run_lint(&fixtures_dir().join("policy")).expect("suite policy bundle has no lint issues");
}
