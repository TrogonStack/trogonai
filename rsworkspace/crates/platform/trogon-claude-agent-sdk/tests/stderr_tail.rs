use trogon_claude_agent_sdk::StderrTail;

#[test]
fn the_default_diagnostic_tail_is_empty_and_debug_reports_only_its_length() {
    let tail = StderrTail::default();
    assert!(tail.as_bytes().is_empty());
    assert_eq!(format!("{tail:?}"), "StderrTail { retained_bytes: 0 }");
}
