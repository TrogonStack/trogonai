#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use serde_json::json;
use trogon_claude_agent_sdk::{AgentDefinition, AgentExecution, Prompt, SettingSource, ToolName};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_minimal_definition_serializes_only_its_required_fields() -> TestResult {
    let agent = AgentDefinition::new("review", Prompt::from("review the change".to_owned()));
    assert_eq!(
        agent.to_wire_json()?,
        json!({"description": "review", "prompt": "review the change"})
    );
    Ok(())
}

#[test]
fn every_optional_field_reaches_the_wire_form() -> TestResult {
    for (memory, literal) in [
        (SettingSource::User, "user"),
        (SettingSource::Project, "project"),
        (SettingSource::Local, "local"),
    ] {
        let mut agent = AgentDefinition::new("review", Prompt::from("review the change".to_owned()));
        agent.tools = Some(vec![ToolName::from("Read")]);
        agent.disallowed_tools = Some(vec![ToolName::from("Bash")]);
        agent.model = Some("model".into());
        agent.skills = Some(vec!["skill".into()]);
        agent.memory = Some(memory);
        agent.initial_prompt = Some(Prompt::from("start".to_owned()));
        agent.max_turns = Some(2.into());
        agent.execution = Some(AgentExecution::Background);
        assert_eq!(
            agent.to_wire_json()?,
            json!({
                "description": "review",
                "prompt": "review the change",
                "tools": ["Read"],
                "disallowedTools": ["Bash"],
                "model": "model",
                "skills": ["skill"],
                "memory": literal,
                "initialPrompt": "start",
                "maxTurns": 2,
                "background": true
            })
        );
    }
    Ok(())
}
