//! Core data model for a trogon automation.

use serde::{Deserialize, Serialize};

/// A configured automation: maps an event trigger to a custom agent behaviour.
///
/// Stored as a JSON blob in NATS KV bucket `AUTOMATIONS`, keyed by
/// `{tenant_id}.{id}` to enforce tenant isolation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Automation {
    /// Unique ID (UUID v4).
    pub id: String,

    /// Tenant this automation belongs to.
    pub tenant_id: String,

    /// Human-readable name, e.g. `"PR review — engineering"`.
    pub name: String,

    /// Trigger pattern in the form `"<nats-subject>:<action>"` or just
    /// `"<nats-subject>"` when any action should match.
    ///
    /// Examples:
    /// - `"github.pull_request:opened"` — PR opened or re-opened
    /// - `"github.push"` — any push to any branch
    /// - `"linear.Issue:create"` — new Linear issue created
    pub trigger: String,

    /// User-facing prompt sent to the agent.  The raw event JSON is prepended
    /// automatically so the model always has access to the event payload.
    pub prompt: String,

    /// Built-in tool names the agent is allowed to use.
    /// An empty list means **all** built-in tools are available.
    pub tools: Vec<String>,

    /// Memory file path inside the GitHub repository,
    /// e.g. `".trogon/memory.md"`.  `None` uses the global default.
    pub memory_path: Option<String>,

    /// Additional MCP servers available exclusively to this automation.
    pub mcp_servers: Vec<McpServer>,

    /// When `false` the automation is skipped without being deleted.
    pub enabled: bool,

    /// ISO-8601 creation timestamp.
    pub created_at: String,

    /// ISO-8601 last-updated timestamp.
    pub updated_at: String,
}

/// An MCP server reference stored inside an automation config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpServer {
    /// Short identifier used to prefix tool names.
    pub name: String,
    /// HTTP endpoint of the MCP server.
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Automation {
        Automation {
            id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            tenant_id: "acme".to_string(),
            name: "PR review".to_string(),
            trigger: "github.pull_request:opened".to_string(),
            prompt: "Review the PR.".to_string(),
            tools: vec!["get_pr_diff".to_string()],
            memory_path: Some(".trogon/memory.md".to_string()),
            mcp_servers: vec![McpServer {
                name: "search".to_string(),
                url: "http://localhost:3000".to_string(),
            }],
            enabled: true,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn round_trips_through_json() {
        let a = sample();
        let json = serde_json::to_string(&a).unwrap();
        let b: Automation = serde_json::from_str(&json).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn tenant_id_survives_serialization() {
        let a = sample();
        let v: serde_json::Value = serde_json::to_value(&a).unwrap();
        assert_eq!(v["tenant_id"], "acme");
    }

    #[test]
    fn disabled_automation_serializes_enabled_false() {
        let mut a = sample();
        a.enabled = false;
        let v: serde_json::Value = serde_json::to_value(&a).unwrap();
        assert_eq!(v["enabled"], false);
    }

    #[test]
    fn empty_tools_serializes_as_empty_array() {
        let mut a = sample();
        a.tools = vec![];
        let v: serde_json::Value = serde_json::to_value(&a).unwrap();
        assert_eq!(v["tools"], serde_json::json!([]));
    }

    #[test]
    fn none_memory_path_serializes_as_null() {
        let mut a = sample();
        a.memory_path = None;
        let v: serde_json::Value = serde_json::to_value(&a).unwrap();
        assert!(v["memory_path"].is_null());
    }

    #[test]
    fn different_tenants_are_not_equal() {
        let a = sample();
        let mut b = sample();
        b.tenant_id = "other-org".to_string();
        assert_ne!(a, b);
    }
}
