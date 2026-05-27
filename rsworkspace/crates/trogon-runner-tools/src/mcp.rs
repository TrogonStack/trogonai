//! Shared MCP client wiring for runners.
//!
//! Connects to per-session HTTP MCP servers, initializes them, and returns the
//! advertised tools (as `trogon_tools::ToolDef`, prefixed `{server}__{tool}`)
//! plus a dispatch table the runner consults when the model calls a prefixed
//! tool. Each runner maps the returned `ToolDef`s into its own wire format.
//!
//! Transport is HTTP-only (see `trogon-mcp`); stdio/SSE servers are not
//! connected. The built-in `AskUserQuestion` tool is filtered out so it never
//! shadows a runner's own elicitation tool.

use std::sync::Arc;

use agent_client_protocol::McpServer;
use tracing::{info, warn};
use trogon_tools::ToolDef;

use crate::egress::EgressPolicy;
use crate::session_store::StoredMcpServer;

/// Convert the ACP `mcpServers` field from a `new_session` request into the
/// persisted, transport-neutral `StoredMcpServer` form. HTTP and SSE entries
/// are captured (SSE is stored but only HTTP is actually connectable today);
/// other variants are dropped.
pub fn convert_mcp_servers(servers: &[McpServer]) -> Vec<StoredMcpServer> {
    servers
        .iter()
        .filter_map(|s| match s {
            McpServer::Http(h) => Some(StoredMcpServer {
                name: h.name.clone(),
                url: h.url.clone(),
                headers: h
                    .headers
                    .iter()
                    .map(|hv| (hv.name.clone(), hv.value.clone()))
                    .collect(),
            }),
            McpServer::Sse(s) => Some(StoredMcpServer {
                name: s.name.clone(),
                url: s.url.clone(),
                headers: s
                    .headers
                    .iter()
                    .map(|hv| (hv.name.clone(), hv.value.clone()))
                    .collect(),
            }),
            _ => None,
        })
        .collect()
}

/// Connect to each session MCP server, initialize it, and return tool defs +
/// a dispatch table `(prefixed_name, original_name, client)`.
///
/// Servers denied by the egress `policy`, or that fail `initialize` /
/// `tools/list`, are logged and skipped — a bad server never blocks a prompt.
pub async fn build_session_mcp(
    http: &reqwest::Client,
    servers: &[StoredMcpServer],
    policy: &EgressPolicy,
) -> (
    Vec<ToolDef>,
    Vec<(String, String, Arc<dyn trogon_mcp::McpCallTool>)>,
) {
    let mut tool_defs = Vec::new();
    let mut dispatch = Vec::new();

    for server in servers {
        if !policy.is_allowed(&server.url) {
            warn!(name = %server.name, url = %server.url, "MCP server URL denied by egress policy — skipping");
            continue;
        }

        let client = Arc::new(trogon_mcp::McpClient::new(http.clone(), &server.url));

        if let Err(e) = client.initialize().await {
            warn!(name = %server.name, url = %server.url, error = %e, "MCP server init failed — skipping");
            continue;
        }

        match client.list_tools().await {
            Ok(tools) => {
                let before = tool_defs.len();
                for tool in tools {
                    if tool.name == "AskUserQuestion" {
                        continue;
                    }
                    let prefixed = format!("{}__{}", server.name, tool.name);
                    tool_defs.push(ToolDef {
                        name: prefixed.clone(),
                        description: tool.description,
                        input_schema: tool.input_schema,
                        cache_control: None,
                    });
                    dispatch.push((
                        prefixed,
                        tool.name,
                        client.clone() as Arc<dyn trogon_mcp::McpCallTool>,
                    ));
                }
                info!(name = %server.name, tools = tool_defs.len() - before, "MCP server connected");
            }
            Err(e) => {
                warn!(name = %server.name, error = %e, "Failed to list MCP tools — skipping");
            }
        }
    }

    (tool_defs, dispatch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use serde_json::json;

    fn server(name: &str, url: &str) -> StoredMcpServer {
        StoredMcpServer {
            name: name.to_string(),
            url: url.to_string(),
            headers: vec![],
        }
    }

    /// A healthy server contributes prefixed tool defs and a matching dispatch entry.
    #[tokio::test]
    async fn lists_and_prefixes_tools() {
        let mcp = MockServer::start();
        mcp.mock(|when, then| {
            when.method(POST).body_contains("\"initialize\"");
            then.status(200).json_body(json!({"jsonrpc":"2.0","id":1,"result":{}}));
        });
        mcp.mock(|when, then| {
            when.method(POST).body_contains("tools/list");
            then.status(200).json_body(json!({
                "jsonrpc":"2.0","id":2,
                "result":{"tools":[
                    {"name":"search","description":"Search the web","inputSchema":{"type":"object"}},
                    {"name":"AskUserQuestion","description":"builtin","inputSchema":{"type":"object"}}
                ]}
            }));
        });

        let http = reqwest::Client::new();
        let (defs, dispatch) = build_session_mcp(
            &http,
            &[server("web", &mcp.url("/mcp"))],
            &EgressPolicy::default_safe(),
        )
        .await;

        // AskUserQuestion is filtered out; "search" is prefixed.
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "web__search");
        assert_eq!(dispatch.len(), 1);
        assert_eq!(dispatch[0].0, "web__search");
        assert_eq!(dispatch[0].1, "search");
    }

    /// A server whose URL is denied by egress policy is skipped entirely.
    #[tokio::test]
    async fn egress_denied_server_skipped() {
        let http = reqwest::Client::new();
        let (defs, dispatch) = build_session_mcp(
            &http,
            &[server("blocked", "http://169.254.169.254/mcp")],
            &EgressPolicy::default_safe(),
        )
        .await;
        assert!(defs.is_empty());
        assert!(dispatch.is_empty());
    }

    /// A server that fails the initialize handshake is skipped, not fatal.
    #[tokio::test]
    async fn init_failure_skipped() {
        let mcp = MockServer::start();
        mcp.mock(|when, then| {
            when.method(POST);
            then.status(500);
        });
        let http = reqwest::Client::new();
        let (defs, dispatch) = build_session_mcp(
            &http,
            &[server("bad", &mcp.url("/mcp"))],
            &EgressPolicy::default_safe(),
        )
        .await;
        assert!(defs.is_empty());
        assert!(dispatch.is_empty());
    }
}
