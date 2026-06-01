use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use trogon_tools::ToolDef;
use trogon_mcp::McpCallTool;

const SPAWN_TIMEOUT: Duration = Duration::from_secs(120);

/// Implements `spawn_agent` by sending a NATS request-reply to
/// `{prefix}.agent.spawn`. The registry resolves the correct agent type
/// from the request payload and returns its output as the tool result.
pub struct SpawnAgentTool {
    nats: async_nats::Client,
    prefix: String,
    session_id: String,
}

impl SpawnAgentTool {
    pub fn new(
        nats: async_nats::Client,
        prefix: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            nats,
            prefix: prefix.into(),
            session_id: session_id.into(),
        }
    }

    pub fn tool_def() -> ToolDef {
        Self::tool_def_with_agents(&[])
    }

    /// Tool definition, optionally listing the custom subagents available in this
    /// project (from `.claude/agents/`) so the model knows which `agent` names it
    /// can delegate to.
    pub fn tool_def_with_agents(agent_names: &[String]) -> ToolDef {
        let mut description = "Spawn a specialised sub-agent in an isolated worktree and return \
            its output. Provide `prompt` (the task). Optionally set `agent` to delegate to a \
            named custom subagent defined in .claude/agents/, or `capability` (e.g. 'explore', \
            'plan')."
            .to_string();
        if !agent_names.is_empty() {
            description.push_str(" Available custom agents: ");
            description.push_str(&agent_names.join(", "));
            description.push('.');
        }
        ToolDef {
            name: "spawn_agent".to_string(),
            description,
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent": {
                        "type": "string",
                        "description": "Name of a custom subagent from .claude/agents/ to use"
                    },
                    "capability": {
                        "type": "string",
                        "description": "Agent capability to use, e.g. 'explore' or 'plan'"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The task or question to send to the sub-agent"
                    }
                },
                "required": ["prompt"]
            }),
            cache_control: None,
        }
    }

    pub fn into_dispatch(self) -> (String, String, Arc<dyn McpCallTool>) {
        (
            "spawn_agent".to_string(),
            "spawn_agent".to_string(),
            Arc::new(self),
        )
    }
}

impl McpCallTool for SpawnAgentTool {
    fn call_tool<'a>(
        &'a self,
        _name: &'a str,
        arguments: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        let nats = self.nats.clone();
        let prefix = self.prefix.clone();

        Box::pin(async move {
            let prompt = arguments["prompt"]
                .as_str()
                .ok_or_else(|| "missing 'prompt' argument".to_string())?;
            // Both optional: `agent` selects a named .claude/agents/ definition;
            // `capability` is the legacy selector. The handler uses `agent` if set.
            let capability = arguments["capability"].as_str().unwrap_or("");
            let agent = arguments["agent"].as_str().unwrap_or("");

            let payload = serde_json::to_vec(&serde_json::json!({
                "capability": capability,
                "agent": agent,
                "prompt": prompt,
                "session_id": self.session_id,
            }))
            .map_err(|e| e.to_string())?;

            let subject = format!("{prefix}.agent.spawn");

            let msg = tokio::time::timeout(
                SPAWN_TIMEOUT,
                nats.request(subject, payload.into()),
            )
            .await
            .map_err(|_| format!("spawn_agent timed out after {}s", SPAWN_TIMEOUT.as_secs()))?
            .map_err(|e| e.to_string())?;

            String::from_utf8(msg.payload.to_vec()).map_err(|e| e.to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_def_name_is_spawn_agent() {
        let def = SpawnAgentTool::tool_def();
        assert_eq!(def.name, "spawn_agent");
    }

    #[test]
    fn tool_def_requires_prompt_and_offers_agent() {
        let def = SpawnAgentTool::tool_def();
        let required = def.input_schema["required"]
            .as_array()
            .expect("required must be an array");
        let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(names, vec!["prompt"], "only prompt is required");
        // `agent` and `capability` are both optional selectors.
        assert!(def.input_schema["properties"].get("agent").is_some());
        assert!(def.input_schema["properties"].get("capability").is_some());
    }

    #[test]
    fn tool_def_with_agents_lists_names() {
        let def = SpawnAgentTool::tool_def_with_agents(&["reviewer".into(), "planner".into()]);
        assert!(def.description.contains("reviewer"));
        assert!(def.description.contains("planner"));
    }

    #[test]
    fn tool_def_cache_control_is_none() {
        let def = SpawnAgentTool::tool_def();
        assert!(def.cache_control.is_none());
    }

    #[test]
    fn tool_def_description_is_non_empty() {
        let def = SpawnAgentTool::tool_def();
        assert!(!def.description.is_empty());
    }
}
