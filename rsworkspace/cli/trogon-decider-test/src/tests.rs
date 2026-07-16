use super::*;

fn error_expectation(yaml: &str) -> String {
    match serde_yaml::from_str::<Then>(yaml) {
        Ok(Then::Error { error }) => error.expected().unwrap_or_default(),
        _ => String::new(),
    }
}

#[test]
fn then_error_accepts_structured_code() {
    assert_eq!(error_expectation("error:\n  code: already-exists\n"), "already-exists");
}

#[test]
fn then_error_accepts_plain_string() {
    assert_eq!(error_expectation("error: already-exists\n"), "already-exists");
}

fn scenario(when: Option<serde_json::Value>, then: Option<Then>, steps: Option<Vec<Step>>) -> Scenario {
    Scenario {
        name: "scenario under test".to_string(),
        given: Vec::new(),
        when,
        then,
        steps,
    }
}

#[test]
fn steps_normalizes_legacy_when_then_shape() {
    let scenario = scenario(
        Some(serde_json::json!({"@type": "a"})),
        Some(Then::Rejected { rejected: true }),
        None,
    );
    assert_eq!(scenario.steps().expect("valid scenario").len(), 1);
}

#[test]
fn steps_returns_ordered_step_list() {
    let scenario = scenario(
        None,
        None,
        Some(vec![
            Step {
                when: serde_json::json!({"@type": "a"}),
                then: Then::Rejected { rejected: true },
            },
            Step {
                when: serde_json::json!({"@type": "b"}),
                then: Then::Rejected { rejected: false },
            },
        ]),
    );
    assert_eq!(scenario.steps().expect("valid scenario").len(), 2);
}

#[test]
fn steps_rejects_empty_steps_list() {
    let scenario = scenario(None, None, Some(Vec::new()));
    let error = scenario.steps().unwrap_err().to_string();
    assert!(error.contains("empty steps list"), "unexpected error: {error}");
}

#[test]
fn steps_rejects_mixing_steps_and_when_then() {
    let scenario = scenario(
        Some(serde_json::json!({"@type": "a"})),
        Some(Then::Rejected { rejected: true }),
        Some(vec![Step {
            when: serde_json::json!({"@type": "a"}),
            then: Then::Rejected { rejected: true },
        }]),
    );
    let error = scenario.steps().unwrap_err().to_string();
    assert!(error.contains("exactly one of"), "unexpected error: {error}");
}

#[test]
fn steps_rejects_missing_when_and_then() {
    let scenario = scenario(None, None, None);
    let error = scenario.steps().unwrap_err().to_string();
    assert!(error.contains("exactly one of"), "unexpected error: {error}");
}

fn set(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| value.to_string()).collect()
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
