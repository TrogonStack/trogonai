use super::*;
use std::path::Path;

#[test]
fn a_command_line_becomes_command_and_arguments() {
    let launch = AgentLaunch::parse("npx -y @zed-industries/claude-agent-acp").expect("parses");
    assert_eq!(launch.config().command(), Path::new("npx"));
    assert_eq!(launch.config().arguments(), ["-y", "@zed-industries/claude-agent-acp"]);
}

#[test]
fn leading_assignments_become_child_environment() {
    let launch = AgentLaunch::parse("RUST_LOG=debug trogonai-agent acp").expect("parses");
    assert_eq!(launch.config().command(), Path::new("trogonai-agent"));
    assert_eq!(
        launch.config().environment().get("RUST_LOG").map(String::as_str),
        Some("debug")
    );
}

#[test]
fn the_zed_agent_servers_json_shape_is_accepted() {
    let launch =
        AgentLaunch::parse(r#"{"command":"gemini","args":["--acp"],"env":{"GEMINI_MODEL":"x"}}"#).expect("parses");
    assert_eq!(launch.config().command(), Path::new("gemini"));
    assert_eq!(launch.config().arguments(), ["--acp"]);
}

#[test]
fn an_empty_spec_is_rejected() {
    assert!(matches!(AgentLaunch::parse("   "), Err(AgentLaunchError::Empty)));
}

#[test]
fn a_spec_with_only_assignments_is_rejected() {
    assert!(matches!(
        AgentLaunch::parse("ONLY=vars"),
        Err(AgentLaunchError::Invalid(_))
    ));
}

#[test]
fn debug_output_never_contains_environment_values() {
    let launch = AgentLaunch::parse("OPENROUTER_API_KEY=sk-secret trogonai-agent acp").expect("parses");
    let printed = format!("{launch:?}");
    assert!(printed.contains("OPENROUTER_API_KEY"));
    assert!(!printed.contains("sk-secret"));
}
