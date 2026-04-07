//! MCP HTTP JSON-RPC client.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};

use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::debug;

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    REQUEST_ID.fetch_add(1, Ordering::Relaxed)
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
    pub fn new(http: Client, url: impl Into<String>) -> Self {
        Self {
            http,
            url: url.into(),
        }
    }

    /// Perform the MCP `initialize` handshake.
    /// Must be called once before `list_tools` or `call_tool`.
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
        debug!(url = %self.url, "MCP server initialized");
        Ok(())
    }

    /// Retrieve the list of tools the server exposes (`tools/list`).
    pub async fn list_tools(&self) -> Result<Vec<McpTool>, String> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": next_id(),
            "method": "tools/list",
            "params": {}
        });
        let resp = self.rpc(body).await?;
        if let Some(err) = resp.get("error") {
            return Err(format!("MCP tools/list error: {err}"));
        }
        let result: ListToolsResult = serde_json::from_value(resp["result"].clone())
            .map_err(|e| format!("MCP tools/list deserialize error: {e}"))?;
        debug!(url = %self.url, count = result.tools.len(), "MCP tools listed");
        Ok(result.tools)
    }

    /// Call a tool by its original (non-prefixed) name and return the text output.
    pub async fn call_tool(&self, name: &str, arguments: &Value) -> Result<String, String> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": next_id(),
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        });
        let resp = self.rpc(body).await?;
        if let Some(err) = resp.get("error") {
            return Err(format!("MCP tool error: {err}"));
        }
        let result: CallToolResult = serde_json::from_value(resp["result"].clone())
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
