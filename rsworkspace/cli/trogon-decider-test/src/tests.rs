use super::*;

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
