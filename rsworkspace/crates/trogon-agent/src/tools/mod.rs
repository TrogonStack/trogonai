pub mod github;
pub mod linear;
pub mod slack;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use trogon_mcp::McpCallTool;

/// Trait for dispatching a named tool call.
pub trait ToolDispatcher: Send + Sync + 'static {
    fn dispatch<'a>(
        &'a self,
        name: &'a str,
        input: &'a serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = String> + Send + 'a>>;
}

/// Concrete [`ToolDispatcher`] that delegates to the built-in [`dispatch_tool`] function.
pub struct DefaultToolDispatcher {
    ctx: Arc<ToolContext>,
}

impl DefaultToolDispatcher {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

impl ToolDispatcher for DefaultToolDispatcher {
    fn dispatch<'a>(
        &'a self,
        name: &'a str,
        input: &'a serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        let ctx = Arc::clone(&self.ctx);
        let name = name.to_string();
        let input = input.clone();
        Box::pin(async move { dispatch_tool(&ctx, &name, &input).await })
    }
}

#[cfg(test)]
pub mod mock {
    use super::*;

    pub struct MockToolDispatcher {
        pub response: String,
    }

    impl MockToolDispatcher {
        pub fn new(response: impl Into<String>) -> Self {
            Self {
                response: response.into(),
            }
        }
    }

    impl ToolDispatcher for MockToolDispatcher {
        fn dispatch<'a>(
            &'a self,
            _name: &'a str,
            _input: &'a serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
            let resp = self.response.clone();
            Box::pin(async move { resp })
        }
    }

    /// Mock implementation of [`AgentConfig`] for unit tests.
    ///
    /// Pre-load `github_contents` to control what `fetch_github_contents`
    /// returns without any real HTTP calls.
    pub struct MockAgentConfig {
        pub proxy_url: String,
        pub github_token: String,
        pub linear_token: String,
        pub slack_token: String,
        /// Value returned by `fetch_github_contents`. `None` simulates a
        /// 404 / unreachable host.
        pub github_contents: Option<Value>,
    }

    impl Default for MockAgentConfig {
        fn default() -> Self {
            Self {
                proxy_url: "http://proxy.test".to_string(),
                github_token: "tok_github_prod_mock001".to_string(),
                linear_token: String::new(),
                slack_token: String::new(),
                github_contents: None,
            }
        }
    }

    impl AgentConfig for MockAgentConfig {
        fn proxy_url(&self) -> &str {
            &self.proxy_url
        }
        fn github_token(&self) -> &str {
            &self.github_token
        }
        fn linear_token(&self) -> &str {
            &self.linear_token
        }
        fn slack_token(&self) -> &str {
            &self.slack_token
        }

        fn fetch_github_contents<'a>(
            &'a self,
            _url: &'a str,
            _token: &'a str,
        ) -> Pin<Box<dyn Future<Output = Option<Value>> + Send + 'a>> {
            let body = self.github_contents.clone();
            Box::pin(async move { body })
        }

        fn init_mcp_clients<'a>(
            &'a self,
            _servers: &'a [crate::config::McpServerConfig],
        ) -> Pin<Box<dyn Future<Output = (Vec<ToolDef>, Vec<(String, String, Arc<dyn McpCallTool>)>)> + Send + 'a>> {
            Box::pin(async move { (vec![], vec![]) })
        }
    }
}

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

/// Trait abstracting the agent's HTTP configuration dependencies.
///
/// Implemented by [`ToolContext`] for production use.  Tests provide
/// [`mock::MockAgentConfig`] to verify behaviour without real HTTP calls.
pub trait AgentConfig: Send + Sync + 'static {
    fn proxy_url(&self) -> &str;
    fn github_token(&self) -> &str;
    fn linear_token(&self) -> &str;
    fn slack_token(&self) -> &str;

    /// Fetch a resource from the GitHub Contents API via the proxy.
    ///
    /// Returns the parsed JSON body on success, `None` on HTTP error or
    /// non-2xx status.  The concrete implementation uses `reqwest`; tests
    /// can return any pre-canned value without touching the network.
    fn fetch_github_contents<'a>(
        &'a self,
        url: &'a str,
        token: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<Value>> + Send + 'a>>;

    /// Initialise MCP server connections defined in an automation config.
    ///
    /// Returns tool definitions and dispatch entries ready to merge into
    /// an [`AgentLoop`].  Returns empty vecs when no servers are reachable.
    fn init_mcp_clients<'a>(
        &'a self,
        servers: &'a [crate::config::McpServerConfig],
    ) -> Pin<Box<dyn Future<Output = (Vec<ToolDef>, Vec<(String, String, Arc<dyn McpCallTool>)>)> + Send + 'a>>;
}

impl AgentConfig for ToolContext {
    fn proxy_url(&self) -> &str {
        &self.proxy_url
    }
    fn github_token(&self) -> &str {
        &self.github_token
    }
    fn linear_token(&self) -> &str {
        &self.linear_token
    }
    fn slack_token(&self) -> &str {
        &self.slack_token
    }

    fn fetch_github_contents<'a>(
        &'a self,
        url: &'a str,
        token: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<Value>> + Send + 'a>> {
        let http = self.http_client.clone();
        let url = url.to_string();
        let token = token.to_string();
        Box::pin(async move {
            let resp = http
                .get(&url)
                .header("Authorization", format!("Bearer {}", token))
                .header("Accept", "application/vnd.github.v3+json")
                .send()
                .await
                .ok()?;
            if !resp.status().is_success() {
                return None;
            }
            resp.json().await.ok()
        })
    }

    fn init_mcp_clients<'a>(
        &'a self,
        servers: &'a [crate::config::McpServerConfig],
    ) -> Pin<Box<dyn Future<Output = (Vec<ToolDef>, Vec<(String, String, Arc<dyn McpCallTool>)>)> + Send + 'a>> {
        let http = self.http_client.clone();
        let servers: Vec<crate::config::McpServerConfig> = servers.to_vec();
        Box::pin(async move { crate::runner::init_mcp_servers(&http, &servers).await })
    }
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
        tool_def(
            "get_pr_diff",
            "Get the unified diff of a pull request.",
            json!({"type":"object","required":["owner","repo","pr_number"],
                   "properties":{"owner":{"type":"string"},"repo":{"type":"string"},
                                 "pr_number":{"type":"integer"}}}),
        ),
        tool_def(
            "list_pr_files",
            "List the files changed in a pull request.",
            json!({"type":"object","required":["owner","repo","pr_number"],
                   "properties":{"owner":{"type":"string"},"repo":{"type":"string"},
                                 "pr_number":{"type":"integer"}}}),
        ),
        tool_def(
            "get_pr_comments",
            "Get all comments on a pull request.",
            json!({"type":"object","required":["owner","repo","pr_number"],
                   "properties":{"owner":{"type":"string"},"repo":{"type":"string"},
                                 "pr_number":{"type":"integer"}}}),
        ),
        tool_def(
            "post_pr_comment",
            "Post a comment on a pull request.",
            json!({"type":"object","required":["owner","repo","pr_number","body"],
                   "properties":{"owner":{"type":"string"},"repo":{"type":"string"},
                                 "pr_number":{"type":"integer"},"body":{"type":"string"}}}),
        ),
        tool_def(
            "get_file_contents",
            "Read the contents of a file at a specific git ref. Returns JSON with `sha` and `content`.",
            json!({"type":"object","required":["owner","repo","path"],
                   "properties":{"owner":{"type":"string"},"repo":{"type":"string"},
                                 "path":{"type":"string"},"ref":{"type":"string"}}}),
        ),
        tool_def(
            "update_file",
            "Create or update a file in the repository.",
            json!({"type":"object","required":["owner","repo","path","message","content"],
                   "properties":{"owner":{"type":"string"},"repo":{"type":"string"},
                                 "path":{"type":"string"},"message":{"type":"string"},
                                 "content":{"type":"string"},"branch":{"type":"string"},
                                 "sha":{"type":"string"}}}),
        ),
        tool_def(
            "create_pull_request",
            "Open a pull request.",
            json!({"type":"object","required":["owner","repo","title","head"],
                   "properties":{"owner":{"type":"string"},"repo":{"type":"string"},
                                 "title":{"type":"string"},"head":{"type":"string"},
                                 "base":{"type":"string"},"body":{"type":"string"}}}),
        ),
        tool_def(
            "request_reviewers",
            "Request reviewers on a pull request.",
            json!({"type":"object","required":["owner","repo","pr_number","reviewers"],
                   "properties":{"owner":{"type":"string"},"repo":{"type":"string"},
                                 "pr_number":{"type":"integer"},
                                 "reviewers":{"type":"array","items":{"type":"string"}}}}),
        ),
        // ── Linear ────────────────────────────────────────────────────────────
        tool_def(
            "get_linear_issue",
            "Fetch a Linear issue by ID, including state, assignee, labels, and team.",
            json!({"type":"object","required":["issue_id"],
                   "properties":{"issue_id":{"type":"string"}}}),
        ),
        tool_def(
            "get_linear_comments",
            "Get all comments on a Linear issue.",
            json!({"type":"object","required":["issue_id"],
                   "properties":{"issue_id":{"type":"string"}}}),
        ),
        tool_def(
            "post_linear_comment",
            "Post a comment on a Linear issue.",
            json!({"type":"object","required":["issue_id","body"],
                   "properties":{"issue_id":{"type":"string"},"body":{"type":"string"}}}),
        ),
        tool_def(
            "update_linear_issue",
            "Update a Linear issue's state, assignee, or priority.",
            json!({"type":"object","required":["issue_id"],
                   "properties":{"issue_id":{"type":"string"},"state_id":{"type":"string"},
                                 "assignee_id":{"type":"string"},"priority":{"type":"integer"}}}),
        ),
    ];
    tools.extend(slack::slack_tool_defs());
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
            "get_pr_diff",
            "list_pr_files",
            "post_pr_comment",
            "get_file_contents",
            "update_file",
            "create_pull_request",
            "request_reviewers",
            "get_pr_comments",
            "get_linear_issue",
            "get_linear_comments",
            "post_linear_comment",
            "update_linear_issue",
            "send_slack_message",
            "read_slack_channel",
        ] {
            assert!(names.contains(expected), "missing tool: {expected}");
        }
    }

    #[test]
    fn all_tool_defs_has_fourteen_entries() {
        assert_eq!(all_tool_defs().len(), 14);
    }
}
