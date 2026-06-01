//! MCP server config and stdio bridge lifecycle for the Trogon REPL.

use crate::fs::Fs;
use crate::mcp_oauth::{OAuthStore, StoredToken};
use crate::stdio_mcp_bridge::StdioMcpBridge;
use agent_client_protocol::{HttpHeader, McpServer, McpServerHttp};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

pub const MCP_CONFIG_PATH: &str = "~/.config/trogon/mcp.json";

/// An active MCP connection: `(name, url, headers)`. Headers are empty for stdio
/// servers (the local bridge needs no auth) and carry auth for HTTP servers.
pub type McpConnection = (String, String, Vec<(String, String)>);

/// Transport used to reach an MCP server. Defaults to `Stdio` so existing
/// `mcp.json` files (which predate this field) keep working.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    #[default]
    Stdio,
    Http,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    #[serde(default)]
    pub transport: McpTransport,
    // ── stdio transport ──
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    // ── http transport ──
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// HTTP headers (e.g. `Authorization: Bearer …`) sent to an HTTP MCP server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<(String, String)>,
    /// When true, an OAuth access token (from the auth store, refreshed as needed)
    /// is injected as an `Authorization: Bearer` header at connect time. The token
    /// itself lives in `mcp-auth.json`, never here.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub oauth: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

struct ActiveBridge {
    name: String,
    url: String,
    /// `Some` for stdio servers (the local stdio→HTTP bridge process); `None` for
    /// direct HTTP servers, which have no local process to tear down.
    bridge: Option<StdioMcpBridge>,
}
pub struct McpManager {
    config: McpConfig,
    active: HashMap<String, Vec<ActiveBridge>>,
    pending: Vec<ActiveBridge>,
    /// OAuth tokens for servers marked `oauth: true`, persisted separately.
    oauth: OAuthStore,
    /// HTTP client used for OAuth token refresh at connect time.
    oauth_http: reqwest::Client,
}

impl McpManager {
    pub fn path() -> PathBuf {
        expand_tilde(MCP_CONFIG_PATH)
    }

    pub fn load<F: Fs>(fs: &F) -> Self {
        let config = match fs.read_to_string(&Self::path()) {
            Ok(raw) if !raw.trim().is_empty() => serde_json::from_str(&raw).unwrap_or_default(),
            _ => McpConfig::default(),
        };
        Self {
            config,
            active: HashMap::new(),
            pending: Vec::new(),
            oauth: OAuthStore::load(fs),
            oauth_http: reqwest::Client::new(),
        }
    }

    /// Persist the OAuth token store (`mcp-auth.json`).
    pub fn save_oauth<F: Fs>(&self, fs: &F) -> std::io::Result<()> {
        self.oauth.save(fs)
    }

    /// Store/replace the OAuth token for `name` and mark its server `oauth: true`.
    pub fn set_oauth_token(&mut self, name: &str, token: StoredToken) {
        self.oauth.set(name, token);
        if let Some(s) = self.config.servers.iter_mut().find(|s| s.name == name) {
            s.oauth = true;
        }
    }

    /// URL of the configured HTTP server `name`, if any.
    pub fn http_server_url(&self, name: &str) -> Option<String> {
        self.config
            .servers
            .iter()
            .find(|s| s.name == name && s.transport == McpTransport::Http)
            .map(|s| s.url.clone())
    }

    pub fn save<F: Fs>(&self, fs: &F) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            fs.create_dir_all(dir)?;
        }
        let raw = serde_json::to_string_pretty(&self.config).unwrap_or_else(|_| "{}".into());
        fs.write(&path, raw.as_bytes())
    }

    pub fn configured_servers(&self) -> &[McpServerConfig] {
        &self.config.servers
    }

    pub fn active_for_session(&self, session_id: &str) -> Vec<(String, String)> {
        self.active
            .get(session_id)
            .map(|v| v.iter().map(|b| (b.name.clone(), b.url.clone())).collect())
            .unwrap_or_default()
    }

    /// Spawn configured MCP servers (stdio bridges + direct HTTP); call
    /// [`Self::commit_pending`] after session.new. `fs` is used to persist any
    /// OAuth tokens refreshed during connection.
    pub async fn spawn_pending<F: Fs>(&mut self, fs: &F) -> Vec<McpServer> {
        self.pending.clear();
        let (servers, bridges) = self.spawn_bridges(fs).await;
        self.pending = bridges;
        servers
    }

    pub fn commit_pending(&mut self, session_id: &str) {
        if self.pending.is_empty() {
            return;
        }
        self.active.insert(session_id.to_string(), std::mem::take(&mut self.pending));
    }

    async fn spawn_bridges<F: Fs>(&mut self, fs: &F) -> (Vec<McpServer>, Vec<ActiveBridge>) {
        let mut bridges = Vec::new();
        let mut acp_servers = Vec::new();

        // Clone configs so we can borrow `self.oauth` mutably for token refresh
        // while iterating.
        let configs = self.config.servers.clone();
        for cfg in &configs {
            match cfg.transport {
                McpTransport::Stdio => {
                    let env: Vec<(String, String)> =
                        cfg.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    match StdioMcpBridge::spawn(&cfg.command, &cfg.args, &env).await {
                        Ok(bridge) => {
                            let url = bridge.url.clone();
                            acp_servers.push(McpServer::Http(McpServerHttp::new(
                                cfg.name.clone(),
                                url.clone(),
                            )));
                            bridges.push(ActiveBridge {
                                name: cfg.name.clone(),
                                url,
                                bridge: Some(bridge),
                            });
                        }
                        Err(e) => {
                            eprintln!("warning: MCP server `{}` failed to start: {e}", cfg.name)
                        }
                    }
                }
                McpTransport::Http => {
                    // Direct HTTP server: no local bridge, pass the URL (and any
                    // auth headers) straight to the runner.
                    let mut headers = cfg.headers.clone();
                    if cfg.oauth {
                        match crate::mcp_oauth::ensure_token(
                            &cfg.name,
                            &mut self.oauth,
                            fs,
                            &self.oauth_http,
                        )
                        .await
                        {
                            Ok(token) => {
                                headers.push(("Authorization".into(), format!("Bearer {token}")))
                            }
                            Err(e) => {
                                eprintln!("warning: MCP server `{}` OAuth: {e}", cfg.name);
                                continue;
                            }
                        }
                    }
                    let mut http = McpServerHttp::new(cfg.name.clone(), cfg.url.clone());
                    if !headers.is_empty() {
                        http = http.headers(
                            headers
                                .iter()
                                .map(|(k, v)| HttpHeader::new(k.clone(), v.clone()))
                                .collect(),
                        );
                    }
                    acp_servers.push(McpServer::Http(http));
                    bridges.push(ActiveBridge {
                        name: cfg.name.clone(),
                        url: cfg.url.clone(),
                        bridge: None,
                    });
                }
            }
        }

        (acp_servers, bridges)
    }

    /// Active MCP connections for a session as `(name, url, headers)`. Stdio
    /// servers carry no headers (the local bridge needs no auth); HTTP servers
    /// carry their configured headers. Used to fetch MCP prompts on demand.
    pub fn active_connections(&self, session_id: &str) -> Vec<McpConnection> {
        self.active
            .get(session_id)
            .map(|bridges| {
                bridges
                    .iter()
                    .map(|b| {
                        let headers = self
                            .config
                            .servers
                            .iter()
                            .find(|c| c.name == b.name && c.transport == McpTransport::Http)
                            .map(|c| c.headers.clone())
                            .unwrap_or_default();
                        (b.name.clone(), b.url.clone(), headers)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub async fn shutdown_session(&mut self, session_id: &str) {
        if let Some(bridges) = self.active.remove(session_id) {
            for b in bridges {
                if let Some(bridge) = b.bridge {
                    bridge.shutdown().await;
                }
            }
        }
    }

    pub async fn shutdown_all(&mut self) {
        let ids: Vec<String> = self.active.keys().cloned().collect();
        for id in ids {
            self.shutdown_session(&id).await;
        }
        for b in std::mem::take(&mut self.pending) {
            if let Some(bridge) = b.bridge {
                bridge.shutdown().await;
            }
        }
    }

    pub fn add_server(&mut self, cfg: McpServerConfig) {
        self.config.servers.retain(|s| s.name != cfg.name);
        self.config.servers.push(cfg);
    }

    pub fn remove_server(&mut self, name: &str) -> bool {
        let before = self.config.servers.len();
        self.config.servers.retain(|s| s.name != name);
        before != self.config.servers.len()
    }

    /// Parse the tail of `/mcp add …` into a config entry. Supports:
    ///   /mcp add <name> <command> [args...]
    ///   /mcp add --transport http <name> <url> [--header "Name: Value"]...
    /// `--transport stdio` is the default; `--header`/`-H` may be repeated and is
    /// only meaningful for HTTP transport.
    pub fn parse_add_args(tail: &str) -> Result<McpServerConfig, String> {
        const STDIO_USAGE: &str = "usage: /mcp add <name> <command> [args...]";
        const HTTP_USAGE: &str =
            "usage: /mcp add --transport http <name> <url> [--header \"Name: Value\"]";

        let tokens = shlex::split(tail.trim())
            .ok_or("could not parse arguments (unbalanced quotes?)")?;
        let mut transport = McpTransport::Stdio;
        let mut headers: Vec<(String, String)> = Vec::new();
        let mut positional: Vec<String> = Vec::new();
        let mut oauth = false;

        let mut i = 0;
        while i < tokens.len() {
            match tokens[i].as_str() {
                "--oauth" => {
                    oauth = true;
                    i += 1;
                }
                "--transport" => {
                    let v = tokens
                        .get(i + 1)
                        .ok_or("--transport requires a value (stdio|http)")?;
                    transport = match v.as_str() {
                        "stdio" => McpTransport::Stdio,
                        "http" => McpTransport::Http,
                        other => {
                            return Err(format!(
                                "unknown transport `{other}` (expected stdio or http)"
                            ));
                        }
                    };
                    i += 2;
                }
                "--header" | "-H" => {
                    let v = tokens
                        .get(i + 1)
                        .ok_or("--header requires a value like \"Name: Value\"")?;
                    let (k, val) = v
                        .split_once(':')
                        .ok_or("--header must be in the form \"Name: Value\"")?;
                    headers.push((k.trim().to_string(), val.trim().to_string()));
                    i += 2;
                }
                _ => {
                    positional.push(tokens[i].clone());
                    i += 1;
                }
            }
        }

        match transport {
            McpTransport::Stdio => {
                if oauth {
                    return Err("--oauth is only valid with --transport http".into());
                }
                if positional.len() < 2 {
                    return Err(STDIO_USAGE.into());
                }
                Ok(McpServerConfig {
                    name: positional[0].clone(),
                    transport,
                    command: positional[1].clone(),
                    args: positional[2..].to_vec(),
                    env: HashMap::new(),
                    url: String::new(),
                    headers: Vec::new(),
                    oauth: false,
                })
            }
            McpTransport::Http => {
                if positional.len() != 2 {
                    return Err(HTTP_USAGE.into());
                }
                Ok(McpServerConfig {
                    name: positional[0].clone(),
                    transport,
                    command: String::new(),
                    args: Vec::new(),
                    env: HashMap::new(),
                    url: positional[1].clone(),
                    headers,
                    oauth,
                })
            }
        }
    }
}

pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::mock::MockFs;

    #[test]
    fn parse_add_args_splits_command_and_args() {
        let cfg = McpManager::parse_add_args("github npx -y @modelcontextprotocol/server-github").unwrap();
        assert_eq!(cfg.name, "github");
        assert_eq!(cfg.transport, McpTransport::Stdio);
        assert_eq!(cfg.command, "npx");
        assert_eq!(cfg.args, vec!["-y", "@modelcontextprotocol/server-github"]);
    }

    #[test]
    fn parse_add_args_http_transport_with_url() {
        let cfg = McpManager::parse_add_args("--transport http linear https://mcp.linear.app/sse").unwrap();
        assert_eq!(cfg.name, "linear");
        assert_eq!(cfg.transport, McpTransport::Http);
        assert_eq!(cfg.url, "https://mcp.linear.app/sse");
        assert!(cfg.command.is_empty());
        assert!(cfg.headers.is_empty());
    }

    #[test]
    fn parse_add_args_http_with_auth_header() {
        let cfg = McpManager::parse_add_args(
            "--transport http api https://api.example.com/mcp --header \"Authorization: Bearer tok123\"",
        )
        .unwrap();
        assert_eq!(cfg.transport, McpTransport::Http);
        assert_eq!(cfg.url, "https://api.example.com/mcp");
        assert_eq!(
            cfg.headers,
            vec![("Authorization".to_string(), "Bearer tok123".to_string())]
        );
    }

    #[test]
    fn parse_add_args_http_missing_url_errors() {
        assert!(McpManager::parse_add_args("--transport http onlyname").is_err());
    }

    #[test]
    fn parse_add_args_unknown_transport_errors() {
        assert!(McpManager::parse_add_args("--transport carrier-pigeon x y").is_err());
    }

    #[test]
    fn parse_add_args_http_oauth_flag() {
        let cfg = McpManager::parse_add_args("--transport http linear https://mcp.linear.app/sse --oauth").unwrap();
        assert!(cfg.oauth);
        assert_eq!(cfg.transport, McpTransport::Http);
        assert!(cfg.headers.is_empty());
    }

    #[test]
    fn parse_add_args_oauth_with_stdio_errors() {
        assert!(McpManager::parse_add_args("--oauth name echo").is_err());
    }

    #[test]
    fn http_config_round_trips_and_legacy_stdio_loads() {
        // Legacy stdio JSON (no `transport` field) must still deserialize as Stdio.
        let legacy: McpServerConfig =
            serde_json::from_str(r#"{"name":"old","command":"echo","args":[]}"#).unwrap();
        assert_eq!(legacy.transport, McpTransport::Stdio);
        assert_eq!(legacy.command, "echo");

        // HTTP config round-trips with headers.
        let http = McpServerConfig {
            name: "api".into(),
            transport: McpTransport::Http,
            command: String::new(),
            args: vec![],
            env: HashMap::new(),
            url: "https://api.example.com/mcp".into(),
            headers: vec![("Authorization".into(), "Bearer t".into())],
            oauth: false,
        };
        let json = serde_json::to_string(&http).unwrap();
        let back: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, http);
    }

    #[test]
    fn save_and_load_round_trip() {
        let fs = MockFs::new();
        let mut mgr = McpManager::load(&fs);
        mgr.add_server(McpServerConfig {
            name: "test".into(),
            transport: McpTransport::Stdio,
            command: "echo".into(),
            args: vec![],
            env: HashMap::new(),
            url: String::new(),
            headers: vec![],
            oauth: false,
        });
        mgr.save(&fs).unwrap();
        let loaded = McpManager::load(&fs);
        assert_eq!(loaded.configured_servers().len(), 1);
        assert_eq!(loaded.configured_servers()[0].name, "test");
    }
}
