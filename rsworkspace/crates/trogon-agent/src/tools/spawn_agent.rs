use std::time::Duration;

use super::ToolDef;

const SPAWN_TIMEOUT: Duration = Duration::from_secs(120);

/// Return the [`ToolDef`] for `spawn_agent`.
///
/// Dispatch is performed via NATS request-reply to `{prefix}.agent.spawn`.
/// The registry resolves the best available agent for the requested capability
/// and returns its output as the tool result.
pub fn tool_def() -> ToolDef {
    super::tool_def(
        "spawn_agent",
        "Spawn a specialised sub-agent (e.g. Explore or Plan) and return its output. \
         The registry resolves the best available agent for the requested capability.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "capability": {
                    "type": "string",
                    "description": "Agent capability to invoke, e.g. 'explore' or 'plan'"
                },
                "prompt": {
                    "type": "string",
                    "description": "The task or question to send to the sub-agent"
                }
            },
            "required": ["capability", "prompt"]
        }),
    )
}

/// Dispatch a `spawn_agent` call via NATS request-reply to `{prefix}.agent.spawn`.
///
/// Returns the sub-agent's string output on success.
pub async fn dispatch(
    nats: &async_nats::Client,
    prefix: &str,
    capability: &str,
    prompt: &str,
) -> Result<String, String> {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_def_name() {
        assert_eq!(tool_def().name, "spawn_agent");
    }

    #[test]
    fn tool_def_requires_capability_and_prompt() {
        let def = tool_def();
        let req = def.input_schema["required"]
            .as_array()
            .expect("required must be array");
        let names: Vec<&str> = req.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"capability"));
        assert!(names.contains(&"prompt"));
    }

    #[test]
    fn tool_def_cache_control_is_none() {
        assert!(tool_def().cache_control.is_none());
    }
}
