use a2a_gateway::policy::Tier2CompiledBundle;

use super::*;
use crate::fixture::FixtureSuite;

fn bundle_with_rules(dir: &std::path::Path, rules: &[(&str, &str)]) -> Tier2CompiledBundle {
    for (name, source) in rules {
        std::fs::write(dir.join(format!("{name}.cel")), source).expect("write cel rule");
    }
    Tier2CompiledBundle::load_from_dir(dir).expect("load bundle")
}

fn parse_fixtures(yaml: &str) -> Vec<crate::fixture::Fixture> {
    let suite: FixtureSuite = serde_yaml::from_str(yaml).expect("parse suite");
    suite.fixtures
}

#[test]
fn allow_fixture_matches_allow_only_bundle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = bundle_with_rules(dir.path(), &[("allow_all", "true")]);
    let fixtures = parse_fixtures(
        r#"
suite: s
bundle: b
fixtures:
  - id: allow-case
    input:
      agent: { id: planner }
    expected_verdict: allow
"#,
    );
    let results = run_suite(bundle, &fixtures);
    assert_eq!(results.len(), 1);
    assert!(results[0].passed());
}

#[test]
fn deny_fixture_matches_deny_with_rule_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = bundle_with_rules(dir.path(), &[("deny_all", "false")]);
    let fixtures = parse_fixtures(
        r#"
suite: s
bundle: b
fixtures:
  - id: deny-case
    input:
      agent: { id: planner }
    expected_verdict:
      deny:
        rule: deny_all
"#,
    );
    let results = run_suite(bundle, &fixtures);
    assert!(results[0].passed());
}

#[test]
fn mismatched_expectation_fails_with_descriptive_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = bundle_with_rules(dir.path(), &[("deny_all", "false")]);
    let fixtures = parse_fixtures(
        r#"
suite: s
bundle: b
fixtures:
  - id: wrongly-allow
    input:
      agent: { id: planner }
    expected_verdict: allow
"#,
    );
    let results = run_suite(bundle, &fixtures);
    assert!(!results[0].passed());
    let error = results[0].outcome.as_ref().expect_err("mismatch recorded");
    assert!(error.to_string().contains("expected Allow, got Deny"));
}

#[test]
fn mismatched_rule_name_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = bundle_with_rules(dir.path(), &[("deny_all", "false")]);
    let fixtures = parse_fixtures(
        r#"
suite: s
bundle: b
fixtures:
  - id: wrong-rule-name
    input:
      agent: { id: planner }
    expected_verdict:
      deny:
        rule: some_other_rule
"#,
    );
    let results = run_suite(bundle, &fixtures);
    assert!(!results[0].passed());
}

#[test]
fn report_counts_failures_and_prints_tap_plan() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = bundle_with_rules(dir.path(), &[("allow_all", "true")]);
    let fixtures = parse_fixtures(
        r#"
suite: s
bundle: b
fixtures:
  - id: pass-case
    input:
      agent: { id: planner }
    expected_verdict: allow
  - id: fail-case
    input:
      agent: { id: planner }
    expected_verdict:
      deny:
        rule: nonexistent
"#,
    );
    let results = run_suite(bundle, &fixtures);
    let failures = report("sample suite", &results, OutputFormat::Tap);
    assert_eq!(failures, 1);
}
