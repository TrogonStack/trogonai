use super::*;
#[test]
fn agent_uses_initialize_field_names() -> Result<(), Box<dyn std::error::Error>> {
    let mut agent = AgentDefinition::new("Inspect", Prompt::from("Inspect without modifications"));
    agent.initial_prompt = Some(Prompt::from("Start"));
    agent.execution = Some(AgentExecution::Background);
    agent.mcp_servers = Some(vec![AgentMcpServerSpec::Named(McpServerName::from("local"))]);
    let value = agent.to_wire_json()?;
    assert_eq!(value["initialPrompt"], "Start");
    assert_eq!(value["background"], true);
    assert_eq!(value["mcpServers"][0], "local");
    assert!(value.get("initial_prompt").is_none());
    Ok(())
}
