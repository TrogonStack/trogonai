use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Anthropic tool definition sent in every request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// Set to `{"type":"ephemeral"}` on the last tool to enable prompt caching
    /// for the tool definitions block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<Value>,
}

/// Shared HTTP context available to every tool execution.
pub struct ToolContext {
    /// Base URL of the running `trogon-secret-proxy`.
    pub proxy_url: String,
}

/// Build a [`ToolDef`] from name, description and a JSON Schema object.
pub fn tool_def(name: &str, description: &str, schema: Value) -> ToolDef {
    ToolDef {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: schema,
        cache_control: None,
    }
}

/// Dispatch a tool call by name. Since trogon-agent-core has no built-in
/// business tools, all calls return an unknown-tool error. MCP tools are
/// dispatched directly by the agent loop via `mcp_dispatch`.
pub async fn dispatch_tool(_ctx: &ToolContext, name: &str, _input: &Value) -> String {
    format!("Unknown tool: {name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_def_stores_fields() {
        let t = tool_def(
            "my_tool",
            "Does something",
            json!({"type": "object", "properties": {}}),
        );
        assert_eq!(t.name, "my_tool");
        assert_eq!(t.description, "Does something");
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_error_string() {
        let ctx = ToolContext {
            proxy_url: "http://localhost:8080".to_string(),
        };
        let result = dispatch_tool(&ctx, "nonexistent_tool", &json!({})).await;
        assert!(result.contains("Unknown tool"));
    }
}
