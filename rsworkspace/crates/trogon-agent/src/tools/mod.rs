pub mod github;
pub mod linear;
pub mod slack;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use trogon_mcp::McpCallTool;

/// Raw HTTP response returned by the transport layer.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Trait abstracting the HTTP transport used by tool functions.
/// Production implementation is `reqwest::Client`; tests use `MockHttpClient`.
pub trait HttpClient: Send + Sync + Clone + 'static {
    fn get(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>>;

    fn post(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>>;

    fn put(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>>;
}

impl HttpClient for reqwest::Client {
    fn get(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
        let url = url.to_string();
        let me = self.clone();
        Box::pin(async move {
            let mut req = me.get(&url);
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
            let resp = req.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let body = resp.text().await.map_err(|e| e.to_string())?;
            Ok(HttpResponse { status, body })
        })
    }

    fn post(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
        let url = url.to_string();
        let me = self.clone();
        Box::pin(async move {
            let mut req = me.post(&url).json(&body);
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
            let resp = req.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let body = resp.text().await.map_err(|e| e.to_string())?;
            Ok(HttpResponse { status, body })
        })
    }

    fn put(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
        let url = url.to_string();
        let me = self.clone();
        Box::pin(async move {
            let mut req = me.put(&url).json(&body);
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
            let resp = req.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let body = resp.text().await.map_err(|e| e.to_string())?;
            Ok(HttpResponse { status, body })
        })
    }
}

type McpInitResult = (
    Vec<ToolDef>,
    Vec<(String, String, std::sync::Arc<dyn trogon_mcp::McpCallTool>)>,
);
type McpFactory = dyn Fn(Vec<crate::config::McpServerConfig>) -> Pin<Box<dyn Future<Output = McpInitResult> + Send>>
    + Send
    + Sync;

/// Trait for dispatching a named tool call.
pub trait ToolDispatcher: Send + Sync + 'static {
    fn dispatch<'a>(
        &'a self,
        name: &'a str,
        input: &'a serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = String> + Send + 'a>>;
}

/// Concrete [`ToolDispatcher`] that delegates to the built-in [`dispatch_tool`] function.
pub struct DefaultToolDispatcher<H = reqwest::Client> {
    ctx: Arc<ToolContext<H>>,
}

impl<H: HttpClient> DefaultToolDispatcher<H> {
    pub fn new(ctx: Arc<ToolContext<H>>) -> Self {
        Self { ctx }
    }
}

impl<H: HttpClient> ToolDispatcher for DefaultToolDispatcher<H> {
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
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

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

    /// Mock dispatcher that counts how many times `dispatch` is called.
    ///
    /// Used in recovery tests to verify that cached tool results are replayed
    /// from NATS KV rather than re-executing the tool after a crash.
    pub struct CountingMockToolDispatcher {
        pub call_count: Arc<AtomicU32>,
        pub response: String,
    }

    impl CountingMockToolDispatcher {
        /// Returns `(dispatcher, counter)`. Read `counter.load(Ordering::SeqCst)`
        /// after the run to assert how many times the tool was actually executed.
        pub fn new(response: impl Into<String>) -> (Self, Arc<AtomicU32>) {
            let count = Arc::new(AtomicU32::new(0));
            let dispatcher = Self {
                call_count: Arc::clone(&count),
                response: response.into(),
            };
            (dispatcher, count)
        }
    }

    impl ToolDispatcher for CountingMockToolDispatcher {
        fn dispatch<'a>(
            &'a self,
            _name: &'a str,
            _input: &'a serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
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
        ) -> Pin<
            Box<
                dyn Future<Output = (Vec<ToolDef>, Vec<(String, String, Arc<dyn McpCallTool>)>)>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(async move { (vec![], vec![]) })
        }
    }

    #[derive(Clone, Default)]
    pub struct MockHttpClient {
        responses: Arc<Mutex<VecDeque<Result<HttpResponse, String>>>>,
    }

    impl MockHttpClient {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn enqueue_ok(&self, status: u16, body: impl Into<String>) {
            self.responses.lock().unwrap().push_back(Ok(HttpResponse {
                status,
                body: body.into(),
            }));
        }

        pub fn enqueue_err(&self, msg: impl Into<String>) {
            self.responses.lock().unwrap().push_back(Err(msg.into()));
        }

        fn next(&self) -> Result<HttpResponse, String> {
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err("MockHttpClient: no response enqueued".to_string()))
        }
    }

    impl HttpClient for MockHttpClient {
        fn get(
            &self,
            _url: &str,
            _headers: Vec<(String, String)>,
        ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
            let r = self.next();
            Box::pin(async move { r })
        }

        fn post(
            &self,
            _url: &str,
            _headers: Vec<(String, String)>,
            _body: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
            let r = self.next();
            Box::pin(async move { r })
        }

        fn put(
            &self,
            _url: &str,
            _headers: Vec<(String, String)>,
            _body: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
            let r = self.next();
            Box::pin(async move { r })
        }
    }

    /// Returns a no-op MCP factory (returns empty vecs).
    pub fn no_mcp_factory() -> Arc<McpFactory> {
        Arc::new(|_servers| Box::pin(async { (vec![], vec![]) }))
    }

    impl ToolContext<MockHttpClient> {
        /// Convenience constructor for unit tests.
        pub fn for_test(
            proxy_url: impl Into<String>,
            github_token: impl Into<String>,
            linear_token: impl Into<String>,
            slack_token: impl Into<String>,
        ) -> Self {
            Self {
                http_client: MockHttpClient::new(),
                proxy_url: proxy_url.into(),
                github_token: github_token.into(),
                linear_token: linear_token.into(),
                slack_token: slack_token.into(),
                mcp_factory: no_mcp_factory(),
            }
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
pub struct ToolContext<H = reqwest::Client> {
    pub http_client: H,
    /// Base URL of the running `trogon-secret-proxy`.
    pub proxy_url: String,
    /// Opaque proxy token for the GitHub API.
    pub github_token: String,
    /// Opaque proxy token for the Linear API.
    pub linear_token: String,
    /// Opaque proxy token for the Slack API.
    pub slack_token: String,
    mcp_factory: std::sync::Arc<McpFactory>,
}

impl ToolContext<reqwest::Client> {
    /// Production constructor that wires the real MCP server initializer.
    pub fn new(
        http_client: reqwest::Client,
        proxy_url: String,
        github_token: String,
        linear_token: String,
        slack_token: String,
    ) -> Self {
        let http_for_mcp = http_client.clone();
        Self {
            http_client,
            proxy_url,
            github_token,
            linear_token,
            slack_token,
            mcp_factory: std::sync::Arc::new(move |servers| {
                let http = http_for_mcp.clone();
                Box::pin(async move { crate::runner::init_mcp_servers(&http, &servers).await })
            }),
        }
    }
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
    #[allow(clippy::type_complexity)]
    fn init_mcp_clients<'a>(
        &'a self,
        servers: &'a [crate::config::McpServerConfig],
    ) -> Pin<
        Box<
            dyn Future<Output = (Vec<ToolDef>, Vec<(String, String, Arc<dyn McpCallTool>)>)>
                + Send
                + 'a,
        >,
    >;
}

impl<H: HttpClient> AgentConfig for ToolContext<H> {
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
        let headers = vec![
            ("Authorization".to_string(), format!("Bearer {token}")),
            (
                "Accept".to_string(),
                "application/vnd.github.v3+json".to_string(),
            ),
        ];
        let fut = self.http_client.get(url, headers);
        Box::pin(async move {
            let resp = fut.await.ok()?;
            if resp.status < 200 || resp.status >= 300 {
                return None;
            }
            serde_json::from_str(&resp.body).ok()
        })
    }

    fn init_mcp_clients<'a>(
        &'a self,
        servers: &'a [crate::config::McpServerConfig],
    ) -> Pin<Box<dyn Future<Output = McpInitResult> + Send + 'a>> {
        let servers = servers.to_vec();
        let factory = std::sync::Arc::clone(&self.mcp_factory);
        Box::pin(async move { (factory)(servers).await })
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
pub async fn dispatch_tool<H: HttpClient>(
    ctx: &ToolContext<H>,
    name: &str,
    input: &Value,
) -> String {
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
        let ctx = ToolContext::for_test(
            "http://localhost:8080",
            "tok_github_prod_test01",
            "tok_linear_prod_test01",
            "",
        );
        let result = dispatch_tool(&ctx, "nonexistent_tool", &json!({})).await;
        assert!(result.contains("Unknown tool"));
    }

    /// `get_pr_comments` is routed and returns a Tool error (not "Unknown tool")
    /// when required inputs are missing — confirms the route exists.
    #[tokio::test]
    async fn dispatch_get_pr_comments_routes_correctly() {
        let ctx = ToolContext::for_test(
            "http://localhost:8080",
            "tok_github_prod_test01",
            "tok_linear_prod_test01",
            "",
        );
        let result = dispatch_tool(&ctx, "get_pr_comments", &json!({})).await;
        assert!(result.starts_with("Tool error:"), "got: {result}");
        assert!(!result.contains("Unknown tool"));
    }

    /// `update_file` is routed and returns a Tool error when inputs are missing.
    #[tokio::test]
    async fn dispatch_update_file_routes_correctly() {
        let ctx = ToolContext::for_test(
            "http://localhost:8080",
            "tok_github_prod_test01",
            "tok_linear_prod_test01",
            "",
        );
        let result = dispatch_tool(&ctx, "update_file", &json!({})).await;
        assert!(result.starts_with("Tool error:"), "got: {result}");
        assert!(!result.contains("Unknown tool"));
    }

    /// `create_pull_request` is routed and returns a Tool error when inputs are missing.
    #[tokio::test]
    async fn dispatch_create_pull_request_routes_correctly() {
        let ctx = ToolContext::for_test(
            "http://localhost:8080",
            "tok_github_prod_test01",
            "tok_linear_prod_test01",
            "",
        );
        let result = dispatch_tool(&ctx, "create_pull_request", &json!({})).await;
        assert!(result.starts_with("Tool error:"), "got: {result}");
        assert!(!result.contains("Unknown tool"));
    }

    /// `get_linear_comments` is routed and returns a Tool error when inputs are missing.
    #[tokio::test]
    async fn dispatch_get_linear_comments_routes_correctly() {
        let ctx = ToolContext::for_test(
            "http://localhost:8080",
            "tok_github_prod_test01",
            "tok_linear_prod_test01",
            "",
        );
        let result = dispatch_tool(&ctx, "get_linear_comments", &json!({})).await;
        assert!(result.starts_with("Tool error:"), "got: {result}");
        assert!(!result.contains("Unknown tool"));
    }

    /// `request_reviewers` is routed and returns a Tool error when inputs are missing.
    #[tokio::test]
    async fn dispatch_request_reviewers_routes_correctly() {
        let ctx = ToolContext::for_test(
            "http://localhost:8080",
            "tok_github_prod_test01",
            "tok_linear_prod_test01",
            "",
        );
        let result = dispatch_tool(&ctx, "request_reviewers", &json!({})).await;
        assert!(result.starts_with("Tool error:"), "got: {result}");
        assert!(!result.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn dispatch_send_slack_message_routes_correctly() {
        let ctx = ToolContext::for_test("http://localhost:8080", "", "", "tok_slack_prod_test01");
        let result = dispatch_tool(&ctx, "send_slack_message", &json!({})).await;
        assert!(result.starts_with("Tool error:"), "got: {result}");
        assert!(!result.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn dispatch_read_slack_channel_routes_correctly() {
        let ctx = ToolContext::for_test("http://localhost:8080", "", "", "tok_slack_prod_test01");
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
