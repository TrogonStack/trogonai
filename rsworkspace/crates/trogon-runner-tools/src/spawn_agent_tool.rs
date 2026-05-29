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
}

impl SpawnAgentTool {
    pub fn new(nats: async_nats::Client, prefix: impl Into<String>) -> Self {
        Self {
            nats,
            prefix: prefix.into(),
        }
    }

    pub fn tool_def() -> ToolDef {
        ToolDef {
            name: "spawn_agent".to_string(),
            description: "Spawn a specialised sub-agent (e.g. Explore or Plan) and return its \
                          output. The registry resolves the best available agent for the \
                          requested capability."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "capability": {
                        "type": "string",
                        "description": "Agent capability to use, e.g. 'explore' or 'plan'"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The task or question to send to the sub-agent"
                    }
                },
                "required": ["capability", "prompt"]
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
            let capability = arguments["capability"]
                .as_str()
                .ok_or_else(|| "missing 'capability' argument".to_string())?;
            let prompt = arguments["prompt"]
                .as_str()
                .ok_or_else(|| "missing 'prompt' argument".to_string())?;

            let payload = serde_json::to_vec(&serde_json::json!({
                "capability": capability,
                "prompt": prompt,
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
    fn tool_def_schema_requires_capability_and_prompt() {
        let def = SpawnAgentTool::tool_def();
        let required = def.input_schema["required"]
            .as_array()
            .expect("required must be an array");
        let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"capability"), "must require 'capability'");
        assert!(names.contains(&"prompt"), "must require 'prompt'");
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
