use crate::{EffortLevel, McpServerName, McpServerOptions, ModelName, PermissionMode, Prompt, SettingSource, ToolName};
use serde::Serialize;
use serde_json::Value;
use std::{collections::BTreeMap, num::NonZeroUsize};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentExecution {
    Foreground,
    Background,
}

#[derive(Clone, Debug)]
pub enum AgentMcpServerSpec {
    Named(McpServerName),
    Inline(BTreeMap<McpServerName, McpServerOptions>),
}

#[derive(Clone, Debug)]
pub struct AgentDefinition {
    pub description: String,
    pub prompt: Prompt,
    pub tools: Option<Vec<ToolName>>,
    pub disallowed_tools: Option<Vec<ToolName>>,
    pub model: Option<ModelName>,
    pub skills: Option<Vec<String>>,
    pub memory: Option<SettingSource>,
    pub initial_prompt: Option<Prompt>,
    pub max_turns: Option<NonZeroUsize>,
    pub execution: Option<AgentExecution>,
    pub effort: Option<EffortLevel>,
    pub permission_mode: Option<PermissionMode>,
    pub mcp_servers: Option<Vec<AgentMcpServerSpec>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentDefinitionWire<'a> {
    description: &'a str,
    prompt: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<&'a str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disallowed_tools: Option<Vec<&'a str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skills: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    memory: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    initial_prompt: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_turns: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    background: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    permission_mode: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mcp_servers: Option<Vec<Value>>,
}

impl AgentDefinition {
    pub fn new(description: impl Into<String>, prompt: Prompt) -> Self {
        Self {
            description: description.into(),
            prompt,
            tools: None,
            disallowed_tools: None,
            model: None,
            skills: None,
            memory: None,
            initial_prompt: None,
            max_turns: None,
            execution: None,
            effort: None,
            permission_mode: None,
            mcp_servers: None,
        }
    }

    pub fn to_wire_json(&self) -> Result<Value, serde_json::Error> {
        let servers = self
            .mcp_servers
            .as_ref()
            .map(|servers| {
                servers
                    .iter()
                    .map(|server| match server {
                        AgentMcpServerSpec::Named(name) => Ok(Value::String(name.as_str().to_owned())),
                        AgentMcpServerSpec::Inline(servers) => servers
                            .iter()
                            .map(|(name, config)| Ok((name.as_str().to_owned(), config.to_wire_json()?)))
                            .collect::<Result<serde_json::Map<String, Value>, serde_json::Error>>()
                            .map(Value::Object),
                    })
                    .collect::<Result<Vec<_>, serde_json::Error>>()
            })
            .transpose()?;
        serde_json::to_value(AgentDefinitionWire {
            description: &self.description,
            prompt: self.prompt.as_str(),
            tools: self
                .tools
                .as_ref()
                .map(|tools| tools.iter().map(ToolName::as_str).collect()),
            disallowed_tools: self
                .disallowed_tools
                .as_ref()
                .map(|tools| tools.iter().map(ToolName::as_str).collect()),
            model: self.model.as_ref().map(ModelName::as_str),
            skills: self.skills.as_deref(),
            memory: self.memory.map(SettingSource::as_str),
            initial_prompt: self.initial_prompt.as_ref().map(Prompt::as_str),
            max_turns: self.max_turns.map(NonZeroUsize::get),
            background: self.execution.map(|execution| execution == AgentExecution::Background),
            effort: self.effort.map(EffortLevel::as_str),
            permission_mode: self.permission_mode.as_ref().map(PermissionMode::as_str),
            mcp_servers: servers,
        })
    }
}

#[cfg(test)]
#[path = "agent_definition_tests.rs"]
mod tests;
