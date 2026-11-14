use crate::{
    ArgvStyle, EffortLevel, JsonObject, McpServerName, McpServerOptions, ModelName, PermissionMode, Prompt,
    SettingSource, ToolName,
};
use indexmap::IndexMap;
use serde_json::{Number, Value};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentExecution {
    Foreground,
    Background,
}

#[derive(Clone, Debug)]
pub enum AgentMcpServerSpec {
    Named(McpServerName),
    /// Servers in insertion order, like the object literal either SDK sends.
    Inline(IndexMap<McpServerName, McpServerOptions>),
    /// Any other entry, sent as given: neither SDK checks the shape before forwarding it.
    Raw(Value),
}

/// TypeScript `effort?: EffortLevel | number`, Python `effort: EffortLevel | int`.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentEffort {
    Level(EffortLevel),
    /// Sent as given, so fractional and out of range values reach the CLI like they do upstream.
    Number(Number),
}

impl From<EffortLevel> for AgentEffort {
    fn from(level: EffortLevel) -> Self {
        Self::Level(level)
    }
}

impl From<Number> for AgentEffort {
    fn from(number: Number) -> Self {
        Self::Number(number)
    }
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
    /// Sent as given: neither SDK rejects zero, negative or fractional values.
    pub max_turns: Option<Number>,
    pub execution: Option<AgentExecution>,
    pub effort: Option<AgentEffort>,
    pub permission_mode: Option<PermissionMode>,
    pub mcp_servers: Option<Vec<AgentMcpServerSpec>>,
    /// TypeScript `criticalSystemReminder_EXPERIMENTAL`. Ignored with [`ArgvStyle::Python`].
    pub critical_system_reminder_experimental: Option<String>,
    /// TypeScript `autoCompactWindow`, sent as given. Ignored with [`ArgvStyle::Python`].
    pub auto_compact_window: Option<Number>,
    /// TypeScript `omitClaudeMd`. Ignored with [`ArgvStyle::Python`].
    pub omit_claude_md: Option<bool>,
    /// TypeScript `observer`. Ignored with [`ArgvStyle::Python`].
    pub observer: Option<String>,
    /// TypeScript `observerMessage`. Ignored with [`ArgvStyle::Python`].
    pub observer_message: Option<String>,
    /// Keys TypeScript forwards after the declared ones because it sends the caller's object as is. A key that
    /// a field above also sets replaces that value in place. Ignored with [`ArgvStyle::Python`], whose
    /// dataclass cannot carry undeclared keys.
    pub extra: JsonObject,
}

/// The declaration order of TypeScript `AgentDefinition`, which a caller writing every field in order sends.
const TYPESCRIPT_ORDER: &[&str] = &[
    "description",
    "tools",
    "disallowedTools",
    "prompt",
    "model",
    "mcpServers",
    "criticalSystemReminder_EXPERIMENTAL",
    "skills",
    "initialPrompt",
    "maxTurns",
    "autoCompactWindow",
    "background",
    "omitClaudeMd",
    "memory",
    "effort",
    "permissionMode",
    "observer",
    "observerMessage",
];

/// The field order of the Python dataclass, which `asdict` keeps when it drops the `None` values.
const PYTHON_ORDER: &[&str] = &[
    "description",
    "prompt",
    "tools",
    "disallowedTools",
    "model",
    "skills",
    "memory",
    "mcpServers",
    "initialPrompt",
    "maxTurns",
    "background",
    "effort",
    "permissionMode",
];

fn tool_list(tools: Option<&Vec<ToolName>>) -> Option<Value> {
    tools.map(|tools| tools.iter().map(|tool| Value::from(tool.as_str())).collect())
}

fn text(value: Option<&str>) -> Option<Value> {
    value.map(Value::from)
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
            critical_system_reminder_experimental: None,
            auto_compact_window: None,
            omit_claude_md: None,
            observer: None,
            observer_message: None,
            extra: JsonObject::new(),
        }
    }

    /// The object the TypeScript SDK sends for this agent.
    pub fn to_wire_json(&self) -> Result<Value, serde_json::Error> {
        self.to_wire_json_for(ArgvStyle::TypeScript)
    }

    /// The object the SDK that `style` names sends for this agent, keys in that SDK's order.
    pub fn to_wire_json_for(&self, style: ArgvStyle) -> Result<Value, serde_json::Error> {
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
                            .collect::<Result<JsonObject, serde_json::Error>>()
                            .map(Value::Object),
                        AgentMcpServerSpec::Raw(value) => Ok(value.clone()),
                    })
                    .collect::<Result<Vec<_>, serde_json::Error>>()
            })
            .transpose()?;
        let mut fields: Vec<(&str, Option<Value>)> = vec![
            ("description", text(Some(self.description.as_str()))),
            ("tools", tool_list(self.tools.as_ref())),
            ("disallowedTools", tool_list(self.disallowed_tools.as_ref())),
            ("prompt", text(Some(self.prompt.as_str()))),
            ("model", text(self.model.as_ref().map(ModelName::as_str))),
            ("mcpServers", servers.map(Value::Array)),
            (
                "criticalSystemReminder_EXPERIMENTAL",
                text(self.critical_system_reminder_experimental.as_deref()),
            ),
            (
                "skills",
                self.skills
                    .as_ref()
                    .map(|skills| skills.iter().map(String::as_str).collect()),
            ),
            ("initialPrompt", text(self.initial_prompt.as_ref().map(Prompt::as_str))),
            ("maxTurns", self.max_turns.clone().map(Value::Number)),
            ("autoCompactWindow", self.auto_compact_window.clone().map(Value::Number)),
            (
                "background",
                self.execution
                    .map(|execution| Value::Bool(execution == AgentExecution::Background)),
            ),
            ("omitClaudeMd", self.omit_claude_md.map(Value::Bool)),
            ("memory", text(self.memory.map(SettingSource::as_str))),
            (
                "effort",
                self.effort.as_ref().map(|effort| match effort {
                    AgentEffort::Level(level) => Value::from(level.as_str()),
                    AgentEffort::Number(number) => Value::Number(number.clone()),
                }),
            ),
            (
                "permissionMode",
                text(self.permission_mode.as_ref().map(PermissionMode::as_str)),
            ),
            ("observer", text(self.observer.as_deref())),
            ("observerMessage", text(self.observer_message.as_deref())),
        ];
        let order = match style {
            ArgvStyle::TypeScript => TYPESCRIPT_ORDER,
            ArgvStyle::Python => PYTHON_ORDER,
        };
        let mut wire = JsonObject::new();
        for key in order {
            if let Some(value) = fields
                .iter_mut()
                .find(|(field, _)| field == key)
                .and_then(|(_, value)| value.take())
            {
                wire.insert((*key).to_owned(), value);
            }
        }
        if style == ArgvStyle::TypeScript {
            wire.extend(self.extra.iter().map(|(key, value)| (key.clone(), value.clone())));
        }
        Ok(Value::Object(wire))
    }
}

#[cfg(test)]
#[path = "agent_definition_tests.rs"]
mod tests;
