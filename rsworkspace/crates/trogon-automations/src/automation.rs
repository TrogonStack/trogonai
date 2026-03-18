//! Core data model for a trogon automation.

use serde::{Deserialize, Serialize};

/// Visibility scope of an automation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Only visible to the owner / within the tenant.
    #[default]
    Private,
    /// Visible to all members of the tenant.
    Public,
}

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
    /// - `"github.pull_request:draft_opened"` — Draft PR opened
    /// - `"github.push"` — any push to any branch
    /// - `"linear.Issue:create"` — new Linear issue created
    /// - `"cron.my-job-id"` — specific scheduled job tick
    /// - `"cron"` — any scheduled job tick
    pub trigger: String,

    /// User-facing prompt sent to the agent.  The raw event JSON is prepended
    /// automatically so the model always has access to the event payload.
    pub prompt: String,

    /// Anthropic model override for this automation.
    /// When `None`, the runner's global model is used.
    #[serde(default)]
    pub model: Option<String>,

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

    /// Visibility scope — `private` (default) or `public`.
    #[serde(default)]
    pub visibility: Visibility,

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
            model: None,
            tools: vec!["get_pr_diff".to_string()],
            memory_path: Some(".trogon/memory.md".to_string()),
            mcp_servers: vec![McpServer {
                name: "search".to_string(),
                url: "http://localhost:3000".to_string(),
            }],
            enabled: true,
            visibility: Visibility::Private,
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

    #[test]
    fn model_none_serializes_as_null() {
        let a = sample();
        let v: serde_json::Value = serde_json::to_value(&a).unwrap();
        assert!(v["model"].is_null());
    }

    #[test]
    fn model_some_survives_round_trip() {
        let mut a = sample();
        a.model = Some("claude-haiku-4-5-20251001".to_string());
        let json = serde_json::to_string(&a).unwrap();
        let b: Automation = serde_json::from_str(&json).unwrap();
        assert_eq!(b.model.as_deref(), Some("claude-haiku-4-5-20251001"));
    }

    #[test]
    fn model_defaults_to_none_when_field_missing_in_json() {
        let json = r#"{"id":"x","tenant_id":"t","name":"n","trigger":"github.push",
                       "prompt":"p","tools":[],"memory_path":null,"mcp_servers":[],
                       "enabled":true,"created_at":"2026-01-01T00:00:00Z",
                       "updated_at":"2026-01-01T00:00:00Z"}"#;
        let a: Automation = serde_json::from_str(json).unwrap();
        assert!(a.model.is_none());
    }

    #[test]
    fn visibility_defaults_to_private() {
        let a = sample();
        assert_eq!(a.visibility, Visibility::Private);
    }

    #[test]
    fn visibility_public_round_trips() {
        let mut a = sample();
        a.visibility = Visibility::Public;
        let json = serde_json::to_string(&a).unwrap();
        let b: Automation = serde_json::from_str(&json).unwrap();
        assert_eq!(b.visibility, Visibility::Public);
    }

    #[test]
    fn visibility_defaults_to_private_when_field_missing_in_json() {
        let json = r#"{"id":"x","tenant_id":"t","name":"n","trigger":"github.push",
                       "prompt":"p","tools":[],"memory_path":null,"mcp_servers":[],
                       "enabled":true,"created_at":"2026-01-01T00:00:00Z",
                       "updated_at":"2026-01-01T00:00:00Z"}"#;
        let a: Automation = serde_json::from_str(json).unwrap();
        assert_eq!(a.visibility, Visibility::Private);
    }

    #[test]
    fn mcp_server_round_trips_through_json() {
        let server = McpServer {
            name: "search".to_string(),
            url: "http://mcp.example.com/search".to_string(),
        };
        let json = serde_json::to_string(&server).unwrap();
        let back: McpServer = serde_json::from_str(&json).unwrap();
        assert_eq!(back, server);
        assert_eq!(back.name, "search");
        assert_eq!(back.url, "http://mcp.example.com/search");
    }

    #[test]
    fn mcp_servers_list_survives_round_trip() {
        let mut a = sample();
        a.mcp_servers = vec![
            McpServer { name: "search".to_string(), url: "http://search.local".to_string() },
            McpServer { name: "db".to_string(), url: "http://db.local".to_string() },
        ];
        let json = serde_json::to_string(&a).unwrap();
        let b: Automation = serde_json::from_str(&json).unwrap();
        assert_eq!(b.mcp_servers.len(), 2);
        assert_eq!(b.mcp_servers[0].name, "search");
        assert_eq!(b.mcp_servers[1].name, "db");
    }

    #[test]
    fn empty_mcp_servers_serializes_as_empty_array() {
        let mut a = sample();
        a.mcp_servers = vec![];
        let v: serde_json::Value = serde_json::to_value(&a).unwrap();
        assert_eq!(v["mcp_servers"], serde_json::json!([]));
    }

    #[test]
    fn visibility_enum_default_is_private() {
        assert_eq!(Visibility::default(), Visibility::Private);
    }

    #[test]
    fn visibility_variants_serialize_as_snake_case() {
        let priv_val = serde_json::to_value(Visibility::Private).unwrap();
        let pub_val = serde_json::to_value(Visibility::Public).unwrap();
        assert_eq!(priv_val, "private");
        assert_eq!(pub_val, "public");
    }
}
