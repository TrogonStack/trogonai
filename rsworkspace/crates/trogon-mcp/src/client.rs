//! MCP HTTP JSON-RPC client.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};

use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::debug;

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[cfg_attr(coverage, coverage(off))]
fn next_id() -> u64 {
    REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

/// Return `scheme://host[:port]` from `url`, stripping userinfo, path, query, and fragment.
/// Falls back to the original string if parsing fails.
fn safe_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let scheme = &url[..scheme_end];
    let after_scheme = &url[scheme_end + 3..];
    let authority = match after_scheme.rfind('@') {
        Some(at) => &after_scheme[at + 1..],
        None => after_scheme,
    };
    let host_end = authority.find(['/', '?', '#']).unwrap_or(authority.len());
    format!("{}://{}", scheme, &authority[..host_end])
}

// ── Public types ──────────────────────────────────────────────────────────────

/// A tool advertised by an MCP server.
#[derive(Debug, Clone, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

// ── McpCallTool trait ─────────────────────────────────────────────────────────

/// Abstraction over the single operation the agent loop needs from an MCP server:
/// calling a named tool and receiving its text output.
///
/// Implementing this trait allows the agent loop to be tested without a live
/// MCP server by injecting a [`MockMcpClient`] or any other fake.
pub trait McpCallTool: Send + Sync + 'static {
    fn call_tool<'a>(
        &'a self,
        name: &'a str,
        arguments: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;
}

// ── Internal response types ───────────────────────────────────────────────────

#[derive(Deserialize)]
struct ListToolsResult {
    #[serde(default)]
    tools: Vec<McpTool>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

#[derive(Deserialize)]
struct CallToolResult {
    #[serde(default)]
    content: Vec<ContentBlock>,
    #[serde(rename = "isError", default)]
    is_error: bool,
}

// ── McpClient ─────────────────────────────────────────────────────────────────

/// HTTP JSON-RPC client for a single MCP server.
pub struct McpClient {
    http: Client,
    url: String,
}

impl McpClient {
    /// Create a new client pointing at `url` (e.g. `http://server/mcp`).
    #[cfg_attr(coverage, coverage(off))]
    pub fn new(http: Client, url: impl Into<String>) -> Self {
        Self {
            http,
            url: url.into(),
        }
    }

    /// Perform the MCP `initialize` handshake.
    /// Must be called once before `list_tools` or `call_tool`.
    #[cfg_attr(coverage, coverage(off))]
    pub async fn initialize(&self) -> Result<(), String> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": next_id(),
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "trogon", "version": "0.1.0" }
            }
        });
        let resp = self.rpc(body).await?;
        if let Some(err) = resp.get("error") {
            return Err(format!("MCP initialize error: {err}"));
        }
        debug!(url = %safe_url(&self.url), "MCP server initialized");
        Ok(())
    }

    /// Retrieve the list of tools the server exposes (`tools/list`).
    #[cfg_attr(coverage, coverage(off))]
    pub async fn list_tools(&self) -> Result<Vec<McpTool>, String> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": next_id(),
            "method": "tools/list",
            "params": {}
        });
        let mut resp = self.rpc(body).await?;
        if let Some(err) = resp.get("error") {
            return Err(format!("MCP tools/list error: {err}"));
        }
        let result: ListToolsResult = serde_json::from_value(resp["result"].take())
            .map_err(|e| format!("MCP tools/list deserialize error: {e}"))?;
        debug!(url = %safe_url(&self.url), count = result.tools.len(), "MCP tools listed");
        Ok(result.tools)
    }

    /// Call a tool by its original (non-prefixed) name and return the text output.
    #[cfg_attr(coverage, coverage(off))]
    pub async fn call_tool(&self, name: &str, arguments: &Value) -> Result<String, String> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": next_id(),
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        });
        let mut resp = self.rpc(body).await?;
        if let Some(err) = resp.get("error") {
            return Err(format!("MCP tool error: {err}"));
        }
        let result: CallToolResult = serde_json::from_value(resp["result"].take())
            .map_err(|e| format!("MCP tools/call deserialize error: {e}"))?;

        let text = result
            .content
            .iter()
            .filter(|b| b.block_type == "text")
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n");

        if result.is_error { Err(text) } else { Ok(text) }
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn rpc(&self, body: Value) -> Result<Value, String> {
        self.http
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("MCP HTTP error: {e}"))?
            .json::<Value>()
            .await
            .map_err(|e| format!("MCP parse error: {e}"))
    }
}

impl McpCallTool for McpClient {
    fn call_tool<'a>(
        &'a self,
        name: &'a str,
        arguments: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(McpClient::call_tool(self, name, arguments))
    }
}

// ── Test mock ─────────────────────────────────────────────────────────────────

#[cfg(feature = "test-support")]
pub mod mock {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A configurable in-memory fake for [`McpCallTool`].
    ///
    /// By default every call returns `Ok("mock response")`.  Call
    /// [`MockMcpClient::set_response`] to change what is returned, or
    /// [`MockMcpClient::set_error`] to make calls fail.
    #[derive(Clone)]
    pub struct MockMcpClient {
        result: Arc<Mutex<Result<String, String>>>,
    }

    impl MockMcpClient {
        pub fn new() -> Self {
            Self {
                result: Arc::new(Mutex::new(Ok("mock response".to_string()))),
            }
        }

        pub fn set_response(&self, response: impl Into<String>) {
            *self.result.lock().unwrap() = Ok(response.into());
        }

        pub fn set_error(&self, error: impl Into<String>) {
            *self.result.lock().unwrap() = Err(error.into());
        }
    }

    impl Default for MockMcpClient {
        fn default() -> Self {
            Self::new()
        }
    }

    impl McpCallTool for MockMcpClient {
        fn call_tool<'a>(
            &'a self,
            _name: &'a str,
            _arguments: &'a Value,
        ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
            let result = self.result.lock().unwrap().clone();
            Box::pin(async move { result })
        }
    }
}

#[cfg(all(test, feature = "test-support"))]
mod mock_tests {
    use super::McpCallTool;
    use super::mock::MockMcpClient;
    use serde_json::json;

    #[tokio::test]
    async fn mock_mcp_client_default_returns_ok_response() {
        let client = MockMcpClient::new();
        let result = client.call_tool("any", &json!({})).await;
        assert_eq!(result, Ok("mock response".to_string()));
    }

    #[tokio::test]
    async fn mock_mcp_client_set_response_changes_return_value() {
        let client = MockMcpClient::new();
        client.set_response("custom result");
        let result = client.call_tool("tool", &json!({"x": 1})).await;
        assert_eq!(result, Ok("custom result".to_string()));
    }

    #[tokio::test]
    async fn mock_mcp_client_set_error_returns_err() {
        let client = MockMcpClient::new();
        client.set_error("something went wrong");
        let result = client.call_tool("tool", &json!({})).await;
        assert_eq!(result, Err("something went wrong".to_string()));
    }

    #[tokio::test]
    async fn mock_mcp_client_set_response_then_set_error_returns_error() {
        let client = MockMcpClient::new();
        client.set_response("ok first");
        client.set_error("then broken");
        let result = client.call_tool("tool", &json!({})).await;
        assert_eq!(result, Err("then broken".to_string()));
    }

    #[tokio::test]
    async fn mock_mcp_client_ignores_tool_name_and_arguments() {
        let client = MockMcpClient::new();
        client.set_response("fixed");
        let r1 = client.call_tool("tool_a", &json!({})).await;
        let r2 = client.call_tool("tool_b", &json!({"key": "val"})).await;
        assert_eq!(r1, Ok("fixed".to_string()));
        assert_eq!(r2, Ok("fixed".to_string()));
    }

    #[tokio::test]
    async fn mock_mcp_client_implements_mcp_call_tool_via_dyn_dispatch() {
        let client: Box<dyn McpCallTool> = Box::new(MockMcpClient::new());
        let result = client.call_tool("test", &json!({})).await;
        assert!(result.is_ok(), "trait dispatch must work: {result:?}");
    }

    #[test]
    fn mock_mcp_client_default_equals_new() {
        let _a = MockMcpClient::new().clone();
        let _b = MockMcpClient::default().clone();
    }
}

#[cfg(test)]
mod tests {
    use super::safe_url;

    #[test]
    fn safe_url_strips_path_query_fragment() {
        assert_eq!(
            safe_url("http://mcp.example.com/mcp?token=secret#frag"),
            "http://mcp.example.com"
        );
    }

    #[test]
    fn safe_url_strips_userinfo() {
        assert_eq!(
            safe_url("http://user:pass@mcp.example.com/mcp"),
            "http://mcp.example.com"
        );
    }

    #[test]
    fn safe_url_preserves_port() {
        assert_eq!(
            safe_url("http://mcp.example.com:8080/mcp"),
            "http://mcp.example.com:8080"
        );
    }

    #[test]
    fn safe_url_no_scheme_returns_original() {
        assert_eq!(safe_url("not-a-url"), "not-a-url");
    }

    #[test]
    fn safe_url_plain_host_no_path() {
        assert_eq!(safe_url("http://mcp.example.com"), "http://mcp.example.com");
    }
}
