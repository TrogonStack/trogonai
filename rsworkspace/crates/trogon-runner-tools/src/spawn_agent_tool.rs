use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use trogon_tools::ToolDef;
use trogon_mcp::McpCallTool;

const SPAWN_TIMEOUT: Duration = Duration::from_secs(120);

/// Implements `spawn_agent` by sending a NATS request-reply to
/// `{prefix}.spawn`. The dedicated spawn responder runs the sub-agent and
/// returns its output as the tool result.
///
/// The subject is deliberately `{prefix}.spawn`, NOT `{prefix}.agent.spawn`:
/// the claude runner's global dispatcher subscribes to `{prefix}.agent.>`, so a
/// spawn subject under `agent.` would be delivered to BOTH this dedicated
/// responder and the global ACP dispatcher (which mis-parses it and races back
/// an error reply). Keeping spawn off the `agent.` namespace avoids that.
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
                "session_id": self.session_id,
            }))
            .map_err(|e| e.to_string())?;

            let subject = format!("{prefix}.spawn");

            // Use an explicit per-request timeout. `Client::request` applies the
            // client's default request timeout (10s), which would fire long before
            // a sub-agent's full tool-use loop completes — a `tokio::time::timeout`
            // wrapper around it is useless because the inner 10s wins. `send_request`
            // with `Request::timeout(Some(..))` overrides that default.
            let request = async_nats::Request::new()
                .payload(payload.into())
                .timeout(Some(SPAWN_TIMEOUT));

            let msg = nats
                .send_request(subject, request)
                .await
                .map_err(|e| {
                    if e.kind() == async_nats::client::RequestErrorKind::TimedOut {
                        format!("spawn_agent timed out after {}s", SPAWN_TIMEOUT.as_secs())
                    } else {
                        e.to_string()
                    }
                })?;

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
