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

use std::sync::{Arc, Mutex};

use agent_client_protocol::McpServer;
use tracing::{info, warn};
use trogon_tools::ToolDef;

use crate::egress::EgressPolicy;
use crate::session_store::StoredMcpServer;

/// Dispatch entry: `(prefixed_name, original_name, client)`.
pub type McpDispatch = Vec<(String, String, Arc<dyn trogon_mcp::McpCallTool>)>;

/// Per-session cache of MCP tool defs and dispatch table from [`build_session_mcp`].
///
/// The first [`get_or_build`](Self::get_or_build) connects and lists tools; later calls
/// reuse the cached result until [`invalidate`](Self::invalidate) or the server/policy
/// inputs change (e.g. after MCP respawn updates session servers).
pub struct SessionMcpCache {
    inner: Mutex<Option<CachedSessionMcp>>,
}

struct CachedSessionMcp {
    servers: Vec<StoredMcpServer>,
    policy: EgressPolicy,
    tool_defs: Vec<ToolDef>,
    dispatch: McpDispatch,
}

impl SessionMcpCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    /// Drop cached tools so the next [`get_or_build`](Self::get_or_build) reconnects and
    /// re-lists.
    pub fn invalidate(&self) {
        *self.inner.lock().expect("SessionMcpCache mutex poisoned") = None;
    }

    /// Return cached MCP tools, or build and cache them on first use.
    pub async fn get_or_build(
        &self,
        http: &reqwest::Client,
        servers: &[StoredMcpServer],
        policy: &EgressPolicy,
    ) -> (Vec<ToolDef>, McpDispatch) {
        if let Some(cached) = self.inner.lock().expect("SessionMcpCache mutex poisoned").as_ref()
            && cached.servers == servers
            && cached.policy == *policy
        {
            return (cached.tool_defs.clone(), cached.dispatch.clone());
        }

        let (tool_defs, dispatch) = build_session_mcp(http, servers, policy).await;

        *self.inner.lock().expect("SessionMcpCache mutex poisoned") = Some(CachedSessionMcp {
            servers: servers.to_vec(),
            policy: policy.clone(),
            tool_defs: tool_defs.clone(),
            dispatch: dispatch.clone(),
        });

        (tool_defs, dispatch)
    }
}

impl Default for SessionMcpCache {
    fn default() -> Self {
        Self::new()
    }
}

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
                headers: h.headers.iter().map(|hv| (hv.name.clone(), hv.value.clone())).collect(),
                timeout_secs: timeout_from_meta(h.meta.as_ref()),
            }),
            McpServer::Sse(s) => Some(StoredMcpServer {
                name: s.name.clone(),
                url: s.url.clone(),
                headers: s.headers.iter().map(|hv| (hv.name.clone(), hv.value.clone())).collect(),
                timeout_secs: timeout_from_meta(s.meta.as_ref()),
            }),
            _ => None,
        })
        .collect()
}

/// The `_meta` key under which the client stashes a per-server request timeout.
pub const TIMEOUT_META_KEY: &str = "trogon_timeout_secs";

/// Extract the per-server timeout (seconds) from an ACP `_meta` map, if present.
pub fn timeout_from_meta(meta: Option<&agent_client_protocol::Meta>) -> Option<u64> {
    meta?.get(TIMEOUT_META_KEY)?.as_u64()
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
) -> (Vec<ToolDef>, McpDispatch) {
    let mut tool_defs = Vec::new();
    let mut dispatch = Vec::new();

    for server in servers {
        if !policy.is_allowed(&server.url) {
            warn!(name = %server.name, url = %server.url, "MCP server URL denied by egress policy — skipping");
            continue;
        }

        let client = Arc::new(
            trogon_mcp::McpClient::with_headers(http.clone(), &server.url, server.headers.clone())
                .with_timeout(server.timeout_secs.map(std::time::Duration::from_secs)),
        );

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
                    dispatch.push((prefixed, tool.name, client.clone() as Arc<dyn trogon_mcp::McpCallTool>));
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
            timeout_secs: None,
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
        let (defs, dispatch) =
            build_session_mcp(&http, &[server("web", &mcp.url("/mcp"))], &EgressPolicy::default_safe()).await;

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

    /// `convert_mcp_servers` reads a per-server timeout from the ACP `_meta`.
    #[test]
    fn convert_reads_timeout_from_meta() {
        use agent_client_protocol::{McpServerHttp, McpServerSse};
        let mut meta = serde_json::Map::new();
        meta.insert(TIMEOUT_META_KEY.to_string(), serde_json::json!(45));

        let http = McpServer::Http(McpServerHttp::new("h", "https://h.example/mcp").meta(meta.clone()));
        let sse = McpServer::Sse(McpServerSse::new("s", "https://s.example/sse").meta(meta));
        let plain = McpServer::Http(McpServerHttp::new("p", "https://p.example/mcp"));

        let stored = convert_mcp_servers(&[http, sse, plain]);
        assert_eq!(stored[0].timeout_secs, Some(45));
        assert_eq!(stored[1].timeout_secs, Some(45));
        assert_eq!(stored[2].timeout_secs, None);
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
        let (defs, dispatch) =
            build_session_mcp(&http, &[server("bad", &mcp.url("/mcp"))], &EgressPolicy::default_safe()).await;
        assert!(defs.is_empty());
        assert!(dispatch.is_empty());
    }

    /// First `get_or_build` populates the cache; a second call reuses it (no extra `tools/list`).
    #[tokio::test]
    async fn session_mcp_cache_reuses_on_second_call() {
        let mcp = MockServer::start();
        mcp.mock(|when, then| {
            when.method(POST).body_contains("\"initialize\"");
            then.status(200).json_body(json!({"jsonrpc":"2.0","id":1,"result":{}}));
        });
        let list_mock = mcp.mock(|when, then| {
            when.method(POST).body_contains("tools/list");
            then.status(200).json_body(json!({
                "jsonrpc":"2.0","id":2,
                "result":{"tools":[
                    {"name":"search","description":"Search","inputSchema":{"type":"object"}}
                ]}
            }));
        });

        let http = reqwest::Client::new();
        let servers = [server("web", &mcp.url("/mcp"))];
        let policy = EgressPolicy::default_safe();
        let cache = SessionMcpCache::new();

        let (defs1, _) = cache.get_or_build(&http, &servers, &policy).await;
        let (defs2, _) = cache.get_or_build(&http, &servers, &policy).await;

        assert_eq!(defs1.len(), 1);
        assert_eq!(defs1[0].name, "web__search");
        assert_eq!(defs2.len(), 1);
        assert_eq!(defs2[0].name, "web__search");
        assert_eq!(list_mock.hits(), 1, "tools/list should run once");
    }

    /// `invalidate` forces a fresh `tools/list` on the next `get_or_build`.
    #[tokio::test]
    async fn session_mcp_cache_invalidates() {
        let mcp = MockServer::start();
        mcp.mock(|when, then| {
            when.method(POST).body_contains("\"initialize\"");
            then.status(200).json_body(json!({"jsonrpc":"2.0","id":1,"result":{}}));
        });
        let list_mock = mcp.mock(|when, then| {
            when.method(POST).body_contains("tools/list");
            then.status(200).json_body(json!({
                "jsonrpc":"2.0","id":2,
                "result":{"tools":[
                    {"name":"search","description":"Search","inputSchema":{"type":"object"}}
                ]}
            }));
        });

        let http = reqwest::Client::new();
        let servers = [server("web", &mcp.url("/mcp"))];
        let policy = EgressPolicy::default_safe();
        let cache = SessionMcpCache::new();

        cache.get_or_build(&http, &servers, &policy).await;
        cache.invalidate();
        cache.get_or_build(&http, &servers, &policy).await;

        assert_eq!(list_mock.hits(), 2, "tools/list should run after invalidate");
    }
}
