use trogon_claude_agent_sdk::{
    AgentName, ControlRequestId, DialogKind, MessageId, Prompt, TaskId, ToolRule, ToolUseId,
};

#[test]
fn owned_strings_are_stored_without_modification() {
    let input = || String::from("  owned value\n");
    assert_eq!(AgentName::from(input()).as_str(), input());
    assert_eq!(ControlRequestId::from(input()).as_str(), input());
    assert_eq!(DialogKind::from(input()).as_str(), input());
    assert_eq!(MessageId::from(input()).as_str(), input());
    assert_eq!(Prompt::from(input()).as_str(), input());
    assert_eq!(TaskId::from(input()).as_str(), input());
    assert_eq!(ToolRule::from(input()).as_str(), input());
    assert_eq!(ToolUseId::from(input()).as_str(), input());
}

#[test]
fn owned_and_borrowed_strings_produce_equal_values() {
    assert_eq!(AgentName::from(String::from("agent")), AgentName::from("agent"));
    assert_eq!(MessageId::from(String::from("id")), MessageId::from("id"));
    assert_eq!(TaskId::from(String::from("task")), TaskId::from("task"));
}
