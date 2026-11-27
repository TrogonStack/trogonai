use super::ToolFilter;

fn s(items: &[&str]) -> Vec<String> {
    items.iter().map(|i| (*i).to_string()).collect()
}

#[test]
fn empty_filter_permits_everything() {
    let filter = ToolFilter::default();
    assert!(filter.is_empty());
    assert!(filter.permits("anything"));
}

#[test]
fn allow_restricts_and_deny_wins() {
    let filter = ToolFilter::new(s(&["sandbox_read*", "sandbox_run_command"]), s(&["sandbox_read_file"]));
    assert!(!filter.is_empty());
    assert!(filter.permits("sandbox_run_command"));
    assert!(filter.permits("sandbox_read_range"));
    assert!(!filter.permits("sandbox_read_file"));
    assert!(!filter.permits("sandbox_delete_lease"));
}

#[test]
fn deny_only_keeps_the_rest() {
    let filter = ToolFilter::new(vec![], s(&["sandbox_delete_lease", "sandbox_fork"]));
    assert!(!filter.permits("sandbox_fork"));
    assert!(filter.permits("sandbox_run_command"));
}

#[test]
fn reports_patterns_that_match_nothing() {
    let filter = ToolFilter::new(s(&["sandbox_run_command", "sandbox_typo"]), s(&["nothing*"]));
    let registered = s(&["sandbox_run_command", "sandbox_fork"]);
    assert_eq!(filter.unmatched_patterns(&registered), vec!["sandbox_typo", "nothing*"]);
}
