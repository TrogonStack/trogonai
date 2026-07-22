use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use trogon_mcp::McpCallTool;
use trogon_tools::ToolDef;

/// Maximum wall-clock a spawned sub-agent may run before the in-process driver
/// (the spawn responder) aborts it with a safety-net timeout. The responder
/// (`trogon-acp-runner`) uses this directly so the two sides share one source of
/// truth.
pub const SPAWN_AGENT_SAFETY_TIMEOUT: Duration = Duration::from_secs(3600);

/// Caller-side NATS request timeout. Derived to sit just beyond the responder's
/// safety timeout so the responder's own timeout fires first and returns a clean
/// error, rather than the caller abandoning a sub-agent that is still executing
/// (NEW-23 — previously a hardcoded 120s while the responder ran for up to 1h).
const SPAWN_TIMEOUT: Duration = Duration::from_secs(SPAWN_AGENT_SAFETY_TIMEOUT.as_secs() + 30);

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
    pub fn new(nats: async_nats::Client, prefix: impl Into<String>, session_id: impl Into<String>) -> Self {
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
            named custom subagent defined in .claude/agents/."
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
        ("spawn_agent".to_string(), "spawn_agent".to_string(), Arc::new(self))
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
            let agent = arguments["agent"].as_str().unwrap_or("");

            let payload = serde_json::to_vec(&serde_json::json!({
                "agent": agent,
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

            let msg = nats.send_request(subject, request).await.map_err(|e| {
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
    fn tool_def_requires_prompt_and_offers_agent() {
        let def = SpawnAgentTool::tool_def();
        let required = def.input_schema["required"]
            .as_array()
            .expect("required must be an array");
        let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(names, vec!["prompt"], "only prompt is required");
        assert!(def.input_schema["properties"].get("agent").is_some());
        assert!(
            def.input_schema["properties"].get("capability").is_none(),
            "capability was removed from the schema"
        );
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
