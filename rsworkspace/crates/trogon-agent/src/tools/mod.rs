pub mod github;
pub mod linear;
pub mod slack;

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

/// Shared HTTP context available to every tool implementation.
pub struct ToolContext {
    pub http_client: reqwest::Client,
    /// Base URL of the running `trogon-secret-proxy`.
    pub proxy_url: String,
    /// Opaque proxy token for the GitHub API.
    pub github_token: String,
    /// Opaque proxy token for the Linear API.
    pub linear_token: String,
    /// Opaque proxy token for the Slack API.
    pub slack_token: String,
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

/// Dispatch a tool call by name and return the string output to feed back to
/// the model.  Unknown tool names return an error string instead of panicking
/// so the agent can recover gracefully.
pub async fn dispatch_tool(ctx: &ToolContext, name: &str, input: &Value) -> String {
    let result = match name {
        "get_pr_diff" => github::get_pr_diff(ctx, input).await,
        "get_file_contents" => github::get_file_contents(ctx, input).await,
        "list_pr_files" => github::list_pr_files(ctx, input).await,
        "get_pr_comments" => github::get_pr_comments(ctx, input).await,
        "update_file" => github::update_file(ctx, input).await,
        "create_pull_request" => github::create_pull_request(ctx, input).await,
        "post_pr_comment" => github::post_pr_comment(ctx, input).await,
        "request_reviewers" => github::request_reviewers(ctx, input).await,
        "get_linear_issue" => linear::get_issue(ctx, input).await,
        "update_linear_issue" => linear::update_issue(ctx, input).await,
        "post_linear_comment" => linear::post_comment(ctx, input).await,
        "get_linear_comments" => linear::get_comments(ctx, input).await,
        "send_slack_message" => slack::send_message(ctx, input).await,
        "read_slack_channel" => slack::read_channel(ctx, input).await,
        "EnterPlanMode" => Ok("Entered plan mode".to_string()),
        unknown => Err(format!("Unknown tool: {unknown}")),
    };
    result.unwrap_or_else(|e| format!("Tool error: {e}"))
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
    async fn dispatch_enter_plan_mode_returns_success() {
        let ctx = ToolContext {
            http_client: reqwest::Client::new(),
            proxy_url: "http://localhost:8080".to_string(),
            github_token: String::new(),
            linear_token: String::new(),
            slack_token: String::new(),
        };
        let result = dispatch_tool(&ctx, "EnterPlanMode", &json!({})).await;
        // Must NOT be an error string
        assert!(!result.starts_with("Unknown tool:"), "got: {result}");
        assert!(!result.starts_with("Tool error:"), "got: {result}");
        assert!(result.contains("plan"), "got: {result}");
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_error_string() {
        let ctx = ToolContext {
            http_client: reqwest::Client::new(),
            proxy_url: "http://localhost:8080".to_string(),
            github_token: "tok_github_prod_test01".to_string(),
            linear_token: "tok_linear_prod_test01".to_string(),
            slack_token: String::new(),
        };
        let result = dispatch_tool(&ctx, "nonexistent_tool", &json!({})).await;
        assert!(result.contains("Unknown tool"));
    }

    /// `get_pr_comments` is routed and returns a Tool error (not "Unknown tool")
    /// when required inputs are missing — confirms the route exists.
    #[tokio::test]
    async fn dispatch_get_pr_comments_routes_correctly() {
        let ctx = ToolContext {
            http_client: reqwest::Client::new(),
            proxy_url: "http://localhost:8080".to_string(),
            github_token: "tok_github_prod_test01".to_string(),
            linear_token: "tok_linear_prod_test01".to_string(),
            slack_token: String::new(),
        };
        let result = dispatch_tool(&ctx, "get_pr_comments", &json!({})).await;
        assert!(result.starts_with("Tool error:"), "got: {result}");
        assert!(!result.contains("Unknown tool"));
    }

    /// `update_file` is routed and returns a Tool error when inputs are missing.
    #[tokio::test]
    async fn dispatch_update_file_routes_correctly() {
        let ctx = ToolContext {
            http_client: reqwest::Client::new(),
            proxy_url: "http://localhost:8080".to_string(),
            github_token: "tok_github_prod_test01".to_string(),
            linear_token: "tok_linear_prod_test01".to_string(),
            slack_token: String::new(),
        };
        let result = dispatch_tool(&ctx, "update_file", &json!({})).await;
        assert!(result.starts_with("Tool error:"), "got: {result}");
        assert!(!result.contains("Unknown tool"));
    }

    /// `create_pull_request` is routed and returns a Tool error when inputs are missing.
    #[tokio::test]
    async fn dispatch_create_pull_request_routes_correctly() {
        let ctx = ToolContext {
            http_client: reqwest::Client::new(),
            proxy_url: "http://localhost:8080".to_string(),
            github_token: "tok_github_prod_test01".to_string(),
            linear_token: "tok_linear_prod_test01".to_string(),
            slack_token: String::new(),
        };
        let result = dispatch_tool(&ctx, "create_pull_request", &json!({})).await;
        assert!(result.starts_with("Tool error:"), "got: {result}");
        assert!(!result.contains("Unknown tool"));
    }

    /// `get_linear_comments` is routed and returns a Tool error when inputs are missing.
    #[tokio::test]
    async fn dispatch_get_linear_comments_routes_correctly() {
        let ctx = ToolContext {
            http_client: reqwest::Client::new(),
            proxy_url: "http://localhost:8080".to_string(),
            github_token: "tok_github_prod_test01".to_string(),
            linear_token: "tok_linear_prod_test01".to_string(),
            slack_token: String::new(),
        };
        let result = dispatch_tool(&ctx, "get_linear_comments", &json!({})).await;
        assert!(result.starts_with("Tool error:"), "got: {result}");
        assert!(!result.contains("Unknown tool"));
    }

    /// `request_reviewers` is routed and returns a Tool error when inputs are missing.
    #[tokio::test]
    async fn dispatch_request_reviewers_routes_correctly() {
        let ctx = ToolContext {
            http_client: reqwest::Client::new(),
            proxy_url: "http://localhost:8080".to_string(),
            github_token: "tok_github_prod_test01".to_string(),
            linear_token: "tok_linear_prod_test01".to_string(),
            slack_token: String::new(),
        };
        let result = dispatch_tool(&ctx, "request_reviewers", &json!({})).await;
        assert!(result.starts_with("Tool error:"), "got: {result}");
        assert!(!result.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn dispatch_send_slack_message_routes_correctly() {
        let ctx = ToolContext {
            http_client: reqwest::Client::new(),
            proxy_url: "http://localhost:8080".to_string(),
            github_token: String::new(),
            linear_token: String::new(),
            slack_token: "tok_slack_prod_test01".to_string(),
        };
        let result = dispatch_tool(&ctx, "send_slack_message", &json!({})).await;
        assert!(result.starts_with("Tool error:"), "got: {result}");
        assert!(!result.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn dispatch_read_slack_channel_routes_correctly() {
        let ctx = ToolContext {
            http_client: reqwest::Client::new(),
            proxy_url: "http://localhost:8080".to_string(),
            github_token: String::new(),
            linear_token: String::new(),
            slack_token: "tok_slack_prod_test01".to_string(),
        };
        let result = dispatch_tool(&ctx, "read_slack_channel", &json!({})).await;
        assert!(result.starts_with("Tool error:"), "got: {result}");
        assert!(!result.contains("Unknown tool"));
    }
}

/// Return definitions for every built-in tool — used by the automation runner
/// to build a per-automation tool list filtered by `Automation::tools`.
pub fn all_tool_defs() -> Vec<ToolDef> {
    use serde_json::json;
    let mut tools = vec![
        // ── GitHub ────────────────────────────────────────────────────────────
        tool_def("get_pr_diff",
            "Get the unified diff of a pull request.",
            json!({"type":"object","required":["owner","repo","pr_number"],
                   "properties":{"owner":{"type":"string"},"repo":{"type":"string"},
                                 "pr_number":{"type":"integer"}}})),
        tool_def("list_pr_files",
            "List the files changed in a pull request.",
            json!({"type":"object","required":["owner","repo","pr_number"],
                   "properties":{"owner":{"type":"string"},"repo":{"type":"string"},
                                 "pr_number":{"type":"integer"}}})),
        tool_def("get_pr_comments",
            "Get all comments on a pull request.",
            json!({"type":"object","required":["owner","repo","pr_number"],
                   "properties":{"owner":{"type":"string"},"repo":{"type":"string"},
                                 "pr_number":{"type":"integer"}}})),
        tool_def("post_pr_comment",
            "Post a comment on a pull request.",
            json!({"type":"object","required":["owner","repo","pr_number","body"],
                   "properties":{"owner":{"type":"string"},"repo":{"type":"string"},
                                 "pr_number":{"type":"integer"},"body":{"type":"string"}}})),
        tool_def("get_file_contents",
            "Read the contents of a file at a specific git ref. Returns JSON with `sha` and `content`.",
            json!({"type":"object","required":["owner","repo","path"],
                   "properties":{"owner":{"type":"string"},"repo":{"type":"string"},
                                 "path":{"type":"string"},"ref":{"type":"string"}}})),
        tool_def("update_file",
            "Create or update a file in the repository.",
            json!({"type":"object","required":["owner","repo","path","message","content"],
                   "properties":{"owner":{"type":"string"},"repo":{"type":"string"},
                                 "path":{"type":"string"},"message":{"type":"string"},
                                 "content":{"type":"string"},"branch":{"type":"string"},
                                 "sha":{"type":"string"}}})),
        tool_def("create_pull_request",
            "Open a pull request.",
            json!({"type":"object","required":["owner","repo","title","head"],
                   "properties":{"owner":{"type":"string"},"repo":{"type":"string"},
                                 "title":{"type":"string"},"head":{"type":"string"},
                                 "base":{"type":"string"},"body":{"type":"string"}}})),
        tool_def("request_reviewers",
            "Request reviewers on a pull request.",
            json!({"type":"object","required":["owner","repo","pr_number","reviewers"],
                   "properties":{"owner":{"type":"string"},"repo":{"type":"string"},
                                 "pr_number":{"type":"integer"},
                                 "reviewers":{"type":"array","items":{"type":"string"}}}})),
        // ── Linear ────────────────────────────────────────────────────────────
        tool_def("get_linear_issue",
            "Fetch a Linear issue by ID, including state, assignee, labels, and team.",
            json!({"type":"object","required":["issue_id"],
                   "properties":{"issue_id":{"type":"string"}}})),
        tool_def("get_linear_comments",
            "Get all comments on a Linear issue.",
            json!({"type":"object","required":["issue_id"],
                   "properties":{"issue_id":{"type":"string"}}})),
        tool_def("post_linear_comment",
            "Post a comment on a Linear issue.",
            json!({"type":"object","required":["issue_id","body"],
                   "properties":{"issue_id":{"type":"string"},"body":{"type":"string"}}})),
        tool_def("update_linear_issue",
            "Update a Linear issue's state, assignee, or priority.",
            json!({"type":"object","required":["issue_id"],
                   "properties":{"issue_id":{"type":"string"},"state_id":{"type":"string"},
                                 "assignee_id":{"type":"string"},"priority":{"type":"integer"}}})),
    ];
    tools.extend(slack::slack_tool_defs());
    tools.push(tool_def(
        "EnterPlanMode",
        "Switch the session into plan mode. In plan mode the agent may only read and analyse — \
         it must not write files, run commands, or call any tool that modifies state.",
        json!({"type": "object", "properties": {}}),
    ));
    tools
}

#[cfg(test)]
mod catalog_tests {
    use super::*;

    #[test]
    fn all_tool_defs_contains_expected_names() {
        let defs = all_tool_defs();
        let names: Vec<&str> = defs.iter().map(|t| t.name.as_str()).collect();
        for expected in &[
            "get_pr_diff", "list_pr_files", "post_pr_comment", "get_file_contents",
            "update_file", "create_pull_request", "request_reviewers", "get_pr_comments",
            "get_linear_issue", "get_linear_comments", "post_linear_comment",
            "update_linear_issue", "send_slack_message", "read_slack_channel",
            "EnterPlanMode",
        ] {
            assert!(names.contains(expected), "missing tool: {expected}");
        }
    }

    #[test]
    fn all_tool_defs_has_fifteen_entries() {
        assert_eq!(all_tool_defs().len(), 15);
    }

    #[test]
    fn enter_plan_mode_tool_def_has_empty_schema() {
        let defs = all_tool_defs();
        let def = defs.iter().find(|t| t.name == "EnterPlanMode").unwrap();
        // Input schema must be an object (no required properties)
        assert_eq!(def.input_schema["type"], "object");
        // Description must mention plan mode
        assert!(def.description.to_lowercase().contains("plan"));
    }
}
