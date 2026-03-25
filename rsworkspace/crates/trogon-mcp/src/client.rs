//! MCP HTTP JSON-RPC client.

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
    // Locate "://" to split scheme from the rest.
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let scheme = &url[..scheme_end];
    let after_scheme = &url[scheme_end + 3..];
    // Strip userinfo (user:pass@).
    let authority = match after_scheme.rfind('@') {
        Some(at) => &after_scheme[at + 1..],
        None => after_scheme,
    };
    // Keep only host[:port] — stop at first '/', '?', or '#'.
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
