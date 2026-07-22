use super::*;

#[test]
fn new_wraps_arbitrary_string_unchanged() {
    let id = StatusTransitionId::new("custom-token");
    assert_eq!(id.as_str(), "custom-token");
    assert_eq!(id.to_string(), "custom-token");
}

#[test]
fn from_terminal_uses_idempotency_segment() {
    assert_eq!(
        StatusTransitionId::from_terminal(TerminalPushTaskState::Completed).as_str(),
        "completed"
    );
    assert_eq!(
        StatusTransitionId::from_terminal(TerminalPushTaskState::Failed).as_str(),
        "failed"
    );
}

#[test]
fn debug_renders_tuple_with_inner_value() {
    let id = StatusTransitionId::new("rejected");
    assert!(format!("{id:?}").contains("rejected"));
}
