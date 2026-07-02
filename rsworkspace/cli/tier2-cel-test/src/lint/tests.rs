use super::*;

fn write_bundle(rules: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, source) in rules {
        std::fs::write(dir.path().join(format!("{name}.cel")), source).expect("write cel rule");
    }
    dir
}

#[test]
fn clean_bundle_has_no_findings() {
    let dir = write_bundle(&[
        ("allow_planner", "agent.id == \"planner\""),
        ("deny_guests", "caller.id != \"guest\""),
    ]);
    let report = lint_bundle(dir.path()).expect("lint ok");
    assert!(report.findings.is_empty());
}

#[test]
fn flags_unbound_variable_as_error() {
    let dir = write_bundle(&[("bad", "environment.stage == \"prod\"")]);
    let report = lint_bundle(dir.path()).expect("lint ok");
    assert!(report.has_errors());
    let finding = report.errors().next().expect("one error");
    assert!(finding.message.contains("environment"));
}

#[test]
fn allows_all_five_bound_variables() {
    let dir = write_bundle(&[(
        "uses_all",
        "request.method == \"message/send\" && caller.id == \"c\" && agent.id == \"a\" && task.id == \"t\" && \"h\" in headers",
    )]);
    let report = lint_bundle(dir.path()).expect("lint ok");
    assert!(report.findings.is_empty());
}

#[test]
fn flags_duplicate_rule_bodies_as_warning() {
    let dir = write_bundle(&[
        ("rule_a", "caller.id != \"guest\""),
        ("rule_b", "caller.id != \"guest\""),
    ]);
    let report = lint_bundle(dir.path()).expect("lint ok");
    assert!(!report.has_errors());
    let warnings: Vec<_> = report.warnings().collect();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].message.contains("duplicate rule body"));
    assert_eq!(warnings[0].file.file_name().unwrap(), "rule_b.cel");
}

#[test]
fn flags_contradictory_rule_pair_as_warning() {
    let dir = write_bundle(&[
        ("allow_planner_only", "agent.id == \"planner\""),
        ("deny_planner_only", "!(agent.id == \"planner\")"),
    ]);
    let report = lint_bundle(dir.path()).expect("lint ok");
    let warnings: Vec<_> = report.warnings().collect();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].message.contains("contradicts rule"));
}

#[test]
fn flags_unreachable_rule_after_unconditional_false() {
    let dir = write_bundle(&[("always_false", "false"), ("zzz_never_runs", "agent.id != \"planner\"")]);
    let report = lint_bundle(dir.path()).expect("lint ok");
    let warnings: Vec<_> = report.warnings().collect();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].message.contains("unreachable"));
    assert_eq!(warnings[0].file.file_name().unwrap(), "zzz_never_runs.cel");
}

#[test]
fn rules_before_always_false_are_not_flagged_unreachable() {
    let dir = write_bundle(&[("aaa_first", "agent.id == \"planner\""), ("always_false", "false")]);
    let report = lint_bundle(dir.path()).expect("lint ok");
    assert!(report.warnings().all(|f| !f.message.contains("unreachable")));
}

#[test]
fn parse_error_is_reported_as_lint_error() {
    let dir = write_bundle(&[("broken", "(1 + 2")]);
    let report = lint_bundle(dir.path()).expect("lint ok");
    assert!(report.has_errors());
    assert!(report.errors().next().expect("error present").message.contains("parse"));
}

#[test]
fn ignores_non_cel_files() {
    let dir = write_bundle(&[("rule_a", "true")]);
    std::fs::write(dir.path().join("notes.txt"), "ignore me").expect("write notes");
    let report = lint_bundle(dir.path()).expect("lint ok");
    assert!(report.findings.is_empty());
}

#[test]
fn missing_directory_is_a_lint_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("does-not-exist");
    let err = lint_bundle(&missing).expect_err("missing dir rejected");
    assert!(matches!(err, LintError::ReadDir { .. }));
}
