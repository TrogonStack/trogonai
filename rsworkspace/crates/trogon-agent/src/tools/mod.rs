pub mod github;
pub mod linear;

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
    async fn dispatch_unknown_tool_returns_error_string() {
        let ctx = ToolContext {
            http_client: reqwest::Client::new(),
            proxy_url: "http://localhost:8080".to_string(),
            github_token: "tok_github_prod_test01".to_string(),
            linear_token: "tok_linear_prod_test01".to_string(),
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
        };
        let result = dispatch_tool(&ctx, "get_linear_comments", &json!({})).await;
        assert!(result.starts_with("Tool error:"), "got: {result}");
        assert!(!result.contains("Unknown tool"));
    }
}
