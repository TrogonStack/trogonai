//! MCP server config and stdio bridge lifecycle for the Trogon REPL.

use crate::fs::Fs;
use crate::mcp_oauth::{OAuthStore, StoredToken};
use crate::stdio_mcp_bridge::StdioMcpBridge;
use agent_client_protocol::{
    EnvVariable, HttpHeader, McpServer, McpServerHttp, McpServerSse, McpServerStdio,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const MCP_CONFIG_PATH: &str = "~/.config/trogon/mcp.json";

/// An active MCP connection reachable by the CLI-side HTTP MCP client:
/// `(name, url, headers)`.
pub type McpConnection = (String, String, Vec<(String, String)>);

/// Transport used to reach an MCP server. Defaults to `Stdio` so existing
/// `mcp.json` files (which predate this field) keep working.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    #[default]
    Stdio,
    Http,
    Sse,
}

impl McpTransport {
    /// True for transports reached over a URL (HTTP and SSE) — these carry
    /// headers, support OAuth, and have no local bridge process.
    pub fn is_remote(self) -> bool {
        matches!(self, McpTransport::Http | McpTransport::Sse)
    }
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
    /// Optional per-request timeout in seconds for remote (HTTP/SSE) servers.
    /// Forwarded to the runner via the ACP `_meta` field; the runner bounds every
    /// MCP call to this duration. `None` uses the runner's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
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
        Self::load_for_cli(fs, None, &[], false)
    }

    /// Load MCP config for a CLI run.
    ///
    /// When `strict` is false (default), merges the user config
    /// (`~/.config/trogon/mcp.json`), an optional project `.mcp.json`, and any
    /// `--mcp-config` paths. When `strict` is true, only the CLI paths are used.
    pub fn load_for_cli<F: Fs>(fs: &F, cwd: Option<&Path>, cli_paths: &[PathBuf], strict: bool) -> Self {
        let mut config = McpConfig::default();
        if !strict {
            merge_config(&mut config, read_config_file(fs, &Self::path()));
            if let Some(cwd) = cwd {
                merge_config(&mut config, read_config_file(fs, &cwd.join(".mcp.json")));
            }
        }
        for path in expand_mcp_config_paths(cli_paths) {
            merge_config(&mut config, read_config_file(fs, &path));
        }
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

    /// URL of the configured remote (HTTP or SSE) server `name`, if any. Used by
    /// `/mcp login` to locate the server to authorize.
    pub fn http_server_url(&self, name: &str) -> Option<String> {
        self.config
            .servers
            .iter()
            .find(|s| s.name == name && s.transport.is_remote())
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
        self.active
            .insert(session_id.to_string(), std::mem::take(&mut self.pending));
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
                    // Native stdio (MCP-3): hand the command to the runner, which
                    // drives the server subprocess in-process via StdioMcpClient —
                    // no local stdio→HTTP bridge and no extra localhost hop.
                    let env: Vec<EnvVariable> = cfg
                        .env
                        .iter()
                        .map(|(k, v)| EnvVariable::new(k.clone(), v.clone()))
                        .collect();
                    let timeout_meta = cfg.timeout_secs.map(|secs| {
                        let mut m = serde_json::Map::new();
                        m.insert(
                            trogon_runner_tools::mcp::TIMEOUT_META_KEY.to_string(),
                            serde_json::json!(secs),
                        );
                        m
                    });
                    let stdio = McpServerStdio::new(cfg.name.clone(), cfg.command.clone())
                        .args(cfg.args.clone())
                        .env(env)
                        .meta(timeout_meta);
                    acp_servers.push(McpServer::Stdio(stdio));
                    bridges.push(ActiveBridge {
                        name: cfg.name.clone(),
                        url: format!("stdio:{}", cfg.command),
                        bridge: None,
                    });
                }
                McpTransport::Http | McpTransport::Sse => {
                    // Direct remote server: no local bridge, pass the URL (and any
                    // auth headers) straight to the runner.
                    let mut headers = cfg.headers.clone();
                    if cfg.oauth {
                        match crate::mcp_oauth::ensure_token(&cfg.name, &mut self.oauth, fs, &self.oauth_http).await {
                            Ok(token) => headers.push(("Authorization".into(), format!("Bearer {token}"))),
                            Err(e) => {
                                eprintln!("warning: MCP server `{}` OAuth: {e}", cfg.name);
                                continue;
                            }
                        }
                    }
                    let acp_headers: Vec<HttpHeader> = headers
                        .iter()
                        .map(|(k, v)| HttpHeader::new(k.clone(), v.clone()))
                        .collect();
                    // Forward a per-server timeout to the runner via `_meta`; the
                    // runner reads it back with `timeout_from_meta`.
                    let timeout_meta = cfg.timeout_secs.map(|secs| {
                        let mut m = serde_json::Map::new();
                        m.insert(
                            trogon_runner_tools::mcp::TIMEOUT_META_KEY.to_string(),
                            serde_json::json!(secs),
                        );
                        m
                    });
                    let server = if cfg.transport == McpTransport::Sse {
                        let mut sse = McpServerSse::new(cfg.name.clone(), cfg.url.clone());
                        if !acp_headers.is_empty() {
                            sse = sse.headers(acp_headers);
                        }
                        McpServer::Sse(sse.meta(timeout_meta))
                    } else {
                        let mut http = McpServerHttp::new(cfg.name.clone(), cfg.url.clone());
                        if !acp_headers.is_empty() {
                            http = http.headers(acp_headers);
                        }
                        McpServer::Http(http.meta(timeout_meta))
                    };
                    acp_servers.push(server);
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

    /// Active HTTP/SSE MCP connections for a session as `(name, url, headers)`.
    ///
    /// Native stdio servers are intentionally excluded: the runner owns those
    /// subprocesses directly, so the CLI cannot fetch prompts from them through
    /// the HTTP MCP client.
    pub fn active_connections(&self, session_id: &str) -> Vec<McpConnection> {
        self.active
            .get(session_id)
            .map(|bridges| {
                bridges
                    .iter()
                    .filter_map(|b| {
                        self.config
                            .servers
                            .iter()
                            .find(|c| c.name == b.name && c.transport.is_remote())
                            .map(|c| (b.name.clone(), b.url.clone(), c.headers.clone()))
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
            "usage: /mcp add --transport http|sse <name> <url> [--header \"Name: Value\"] [--timeout <secs>]";

        let tokens = shlex::split(tail.trim()).ok_or("could not parse arguments (unbalanced quotes?)")?;
        let mut transport = McpTransport::Stdio;
        let mut headers: Vec<(String, String)> = Vec::new();
        let mut positional: Vec<String> = Vec::new();
        let mut oauth = false;
        let mut timeout_secs: Option<u64> = None;

        let mut i = 0;
        while i < tokens.len() {
            match tokens[i].as_str() {
                "--oauth" => {
                    oauth = true;
                    i += 1;
                }
                "--timeout" => {
                    let v = tokens.get(i + 1).ok_or("--timeout requires a value in seconds")?;
                    let secs: u64 = v
                        .parse()
                        .map_err(|_| format!("--timeout must be a positive integer, got `{v}`"))?;
                    if secs == 0 {
                        return Err("--timeout must be greater than 0".into());
                    }
                    timeout_secs = Some(secs);
                    i += 2;
                }
                "--transport" => {
                    let v = tokens.get(i + 1).ok_or("--transport requires a value (stdio|http)")?;
                    transport = match v.as_str() {
                        "stdio" => McpTransport::Stdio,
                        "http" => McpTransport::Http,
                        "sse" => McpTransport::Sse,
                        other => {
                            return Err(format!("unknown transport `{other}` (expected stdio, http, or sse)"));
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
                    return Err("--oauth is only valid with --transport http or sse".into());
                }
                if timeout_secs.is_some() {
                    return Err("--timeout is only valid with --transport http or sse".into());
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
                    timeout_secs: None,
                })
            }
            McpTransport::Http | McpTransport::Sse => {
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
                    timeout_secs,
                })
            }
        }
    }

    /// Import MCP servers from a Claude Desktop `claude_desktop_config.json`.
    ///
    /// Each entry under `mcpServers` is converted to a trogon server config:
    /// stdio (`command`/`args`/`env`) and remote (`url`, optional `type: "sse"`)
    /// entries are supported. Existing servers with the same name are replaced.
    /// Entries with neither a command nor a URL are reported as skipped. The
    /// caller is responsible for persisting via [`Self::save`].
    pub fn import_claude_desktop<F: Fs>(&mut self, fs: &F, path: &Path) -> Result<ImportSummary, String> {
        let raw = fs
            .read_to_string(path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        let parsed: ClaudeDesktopConfig =
            serde_json::from_str(&raw).map_err(|e| format!("invalid Claude Desktop config: {e}"))?;

        let mut imported = Vec::new();
        let mut skipped = Vec::new();
        for (name, server) in parsed.mcp_servers {
            match server.into_config(&name) {
                Some(cfg) => {
                    self.add_server(cfg);
                    imported.push(name);
                }
                None => skipped.push(name),
            }
        }
        imported.sort();
        skipped.sort();
        Ok(ImportSummary { imported, skipped })
    }
}

/// Outcome of [`McpManager::import_claude_desktop`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportSummary {
    /// Names of servers added or replaced.
    pub imported: Vec<String>,
    /// Names skipped because the entry had neither a command nor a URL.
    pub skipped: Vec<String>,
}

/// Default Claude Desktop config path for the current platform, if the relevant
/// environment variable (`HOME`, or `APPDATA` on Windows) is set.
pub fn default_claude_desktop_config_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").ok()?;
        Some(PathBuf::from(appdata).join("Claude/claude_desktop_config.json"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let home = PathBuf::from(std::env::var("HOME").ok()?);
        #[cfg(target_os = "macos")]
        let rel = "Library/Application Support/Claude/claude_desktop_config.json";
        #[cfg(not(target_os = "macos"))]
        let rel = ".config/Claude/claude_desktop_config.json";
        Some(home.join(rel))
    }
}

/// Wire shape of a Claude Desktop `claude_desktop_config.json` (only the
/// `mcpServers` map is read; all other keys are ignored).
#[derive(Debug, Deserialize)]
struct ClaudeDesktopConfig {
    #[serde(default, rename = "mcpServers")]
    mcp_servers: HashMap<String, ClaudeDesktopServer>,
}

/// Wire shape of a single Claude Desktop server entry. Both stdio and remote
/// forms are accepted; absent fields default to empty.
#[derive(Debug, Deserialize)]
struct ClaudeDesktopServer {
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    url: String,
    /// Optional explicit transport hint (`"stdio"`, `"http"`, `"sse"`). When a
    /// URL is present and this is `"sse"`, the server is imported as SSE.
    #[serde(default, rename = "type")]
    transport_type: String,
    #[serde(default)]
    headers: HashMap<String, String>,
}

impl ClaudeDesktopServer {
    /// Convert to a trogon [`McpServerConfig`], or `None` if the entry has
    /// neither a command (stdio) nor a URL (remote).
    fn into_config(self, name: &str) -> Option<McpServerConfig> {
        if !self.url.is_empty() {
            let transport = if self.transport_type.eq_ignore_ascii_case("sse") {
                McpTransport::Sse
            } else {
                McpTransport::Http
            };
            let mut headers: Vec<(String, String)> = self.headers.into_iter().collect();
            // HashMap iteration order is unspecified — sort so imports are
            // deterministic (round-trips, tests, diffs).
            headers.sort();
            return Some(McpServerConfig {
                name: name.to_string(),
                transport,
                command: String::new(),
                args: Vec::new(),
                env: HashMap::new(),
                url: self.url,
                headers,
                oauth: false,
                timeout_secs: None,
            });
        }
        if !self.command.is_empty() {
            return Some(McpServerConfig {
                name: name.to_string(),
                transport: McpTransport::Stdio,
                command: self.command,
                args: self.args,
                env: self.env,
                url: String::new(),
                headers: Vec::new(),
                oauth: false,
                timeout_secs: None,
            });
        }
        None
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

fn read_config_file<F: Fs>(fs: &F, path: &Path) -> McpConfig {
    match fs.read_to_string(path) {
        Ok(raw) if !raw.trim().is_empty() => match serde_json::from_str(&raw) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("warning: {} is invalid ({e}) — ignoring", path.display());
                McpConfig::default()
            }
        },
        _ => McpConfig::default(),
    }
}

fn merge_config(into: &mut McpConfig, from: McpConfig) {
    for server in from.servers {
        into.servers.retain(|s| s.name != server.name);
        into.servers.push(server);
    }
}

/// Expand CLI `--mcp-config` values, supporting repeatable flags and comma lists.
pub fn expand_mcp_config_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for path in paths {
        for part in path.to_string_lossy().split(',') {
            let trimmed = part.trim();
            if !trimmed.is_empty() {
                out.push(PathBuf::from(trimmed));
            }
        }
    }
    out
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
        let legacy: McpServerConfig = serde_json::from_str(r#"{"name":"old","command":"echo","args":[]}"#).unwrap();
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
            timeout_secs: None,
        };
        let json = serde_json::to_string(&http).unwrap();
        let back: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, http);
    }

    #[test]
    fn parse_add_args_http_with_timeout() {
        let cfg = McpManager::parse_add_args("--transport http api https://api.example.com/mcp --timeout 30").unwrap();
        assert_eq!(cfg.timeout_secs, Some(30));
    }

    /// MCP-3: a stdio server config is forwarded to the runner as a native
    /// `McpServer::Stdio` (driven in-process), not spawned as a local bridge.
    #[tokio::test]
    async fn stdio_config_yields_native_server_not_bridge() {
        use crate::fs::mock::MockFs;
        let mut env = HashMap::new();
        env.insert("TOKEN".to_string(), "x".to_string());
        let mut mgr = McpManager {
            config: McpConfig {
                servers: vec![McpServerConfig {
                    name: "fs".to_string(),
                    transport: McpTransport::Stdio,
                    command: "/usr/bin/server".to_string(),
                    args: vec!["--root".to_string(), ".".to_string()],
                    env,
                    url: String::new(),
                    headers: vec![],
                    oauth: false,
                    timeout_secs: Some(5),
                }],
            },
            active: HashMap::new(),
            pending: Vec::new(),
            oauth: OAuthStore::default(),
            oauth_http: reqwest::Client::new(),
        };
        let servers = mgr.spawn_pending(&MockFs::new()).await;
        assert_eq!(servers.len(), 1);
        match &servers[0] {
            McpServer::Stdio(s) => {
                assert_eq!(s.command.to_string_lossy(), "/usr/bin/server");
                assert_eq!(s.args, vec!["--root".to_string(), ".".to_string()]);
                assert_eq!(s.env.len(), 1);
                assert_eq!(s.env[0].name, "TOKEN");
            }
            other => panic!("expected native Stdio server, got {other:?}"),
        }
        // No local bridge subprocess was spawned for the stdio server.
        assert!(mgr.pending.iter().all(|b| b.bridge.is_none()));
    }

    #[tokio::test]
    async fn active_connections_excludes_native_stdio_servers() {
        use crate::fs::mock::MockFs;
        let mut mgr = McpManager {
            config: McpConfig {
                servers: vec![
                    McpServerConfig {
                        name: "fs".to_string(),
                        transport: McpTransport::Stdio,
                        command: "/usr/bin/server".to_string(),
                        args: vec![],
                        env: HashMap::new(),
                        url: String::new(),
                        headers: vec![],
                        oauth: false,
                        timeout_secs: None,
                    },
                    McpServerConfig {
                        name: "api".to_string(),
                        transport: McpTransport::Http,
                        command: String::new(),
                        args: vec![],
                        env: HashMap::new(),
                        url: "https://api.example.com/mcp".to_string(),
                        headers: vec![("Authorization".to_string(), "Bearer t".to_string())],
                        oauth: false,
                        timeout_secs: None,
                    },
                ],
            },
            active: HashMap::new(),
            pending: Vec::new(),
            oauth: OAuthStore::default(),
            oauth_http: reqwest::Client::new(),
        };

        let _ = mgr.spawn_pending(&MockFs::new()).await;
        mgr.commit_pending("s1");

        assert_eq!(
            mgr.active_for_session("s1"),
            vec![
                ("fs".to_string(), "stdio:/usr/bin/server".to_string()),
                ("api".to_string(), "https://api.example.com/mcp".to_string()),
            ]
        );
        assert_eq!(
            mgr.active_connections("s1"),
            vec![(
                "api".to_string(),
                "https://api.example.com/mcp".to_string(),
                vec![("Authorization".to_string(), "Bearer t".to_string())],
            )]
        );
    }

    #[test]
    fn parse_add_args_timeout_with_stdio_errors() {
        assert!(McpManager::parse_add_args("--timeout 10 name echo").is_err());
    }

    #[test]
    fn parse_add_args_timeout_zero_errors() {
        assert!(McpManager::parse_add_args("--transport http api https://x/mcp --timeout 0").is_err());
    }

    #[test]
    fn parse_add_args_timeout_non_numeric_errors() {
        assert!(McpManager::parse_add_args("--transport http api https://x/mcp --timeout abc").is_err());
    }

    #[test]
    fn parse_add_args_sse_transport_with_url() {
        let cfg = McpManager::parse_add_args("--transport sse linear https://mcp.linear.app/sse").unwrap();
        assert_eq!(cfg.name, "linear");
        assert_eq!(cfg.transport, McpTransport::Sse);
        assert_eq!(cfg.url, "https://mcp.linear.app/sse");
        assert!(cfg.command.is_empty());
    }

    #[test]
    fn parse_add_args_sse_with_header_and_oauth() {
        let cfg =
            McpManager::parse_add_args("--transport sse api https://api.example.com/sse --oauth -H \"X-Env: prod\"")
                .unwrap();
        assert_eq!(cfg.transport, McpTransport::Sse);
        assert!(cfg.oauth);
        assert_eq!(cfg.headers, vec![("X-Env".to_string(), "prod".to_string())]);
    }

    #[test]
    fn sse_is_remote_and_login_url_resolves() {
        let fs = MockFs::new();
        let mut mgr = McpManager::load(&fs);
        mgr.add_server(McpServerConfig {
            name: "linear".into(),
            transport: McpTransport::Sse,
            command: String::new(),
            args: vec![],
            env: HashMap::new(),
            url: "https://mcp.linear.app/sse".into(),
            headers: vec![],
            oauth: false,
            timeout_secs: None,
        });
        assert!(McpTransport::Sse.is_remote());
        assert_eq!(
            mgr.http_server_url("linear").as_deref(),
            Some("https://mcp.linear.app/sse")
        );
    }

    #[test]
    fn import_claude_desktop_stdio_http_and_sse() {
        let fs = MockFs::new();
        let path = PathBuf::from("/tmp/claude_desktop_config.json");
        fs.write(
            &path,
            br#"{
                "mcpServers": {
                    "github": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"], "env": {"TOKEN": "x"}},
                    "linear": {"type": "sse", "url": "https://mcp.linear.app/sse"},
                    "api": {"url": "https://api.example.com/mcp", "headers": {"Authorization": "Bearer t"}},
                    "broken": {}
                }
            }"#,
        )
        .unwrap();

        let mut mgr = McpManager::load(&fs);
        let summary = mgr.import_claude_desktop(&fs, &path).unwrap();
        assert_eq!(summary.imported, vec!["api", "github", "linear"]);
        assert_eq!(summary.skipped, vec!["broken"]);

        let by_name = |n: &str| mgr.configured_servers().iter().find(|s| s.name == n).cloned().unwrap();
        let gh = by_name("github");
        assert_eq!(gh.transport, McpTransport::Stdio);
        assert_eq!(gh.command, "npx");
        assert_eq!(gh.env.get("TOKEN").map(String::as_str), Some("x"));

        let linear = by_name("linear");
        assert_eq!(linear.transport, McpTransport::Sse);
        assert_eq!(linear.url, "https://mcp.linear.app/sse");

        let api = by_name("api");
        assert_eq!(api.transport, McpTransport::Http);
        assert_eq!(api.headers, vec![("Authorization".to_string(), "Bearer t".to_string())]);
    }

    #[test]
    fn import_claude_desktop_missing_file_errors() {
        let fs = MockFs::new();
        let mut mgr = McpManager::load(&fs);
        assert!(
            mgr.import_claude_desktop(&fs, &PathBuf::from("/tmp/nope.json"))
                .is_err()
        );
    }

    #[test]
    fn import_claude_desktop_replaces_same_name() {
        let fs = MockFs::new();
        let mut mgr = McpManager::load(&fs);
        mgr.add_server(McpServerConfig {
            name: "github".into(),
            transport: McpTransport::Stdio,
            command: "old".into(),
            args: vec![],
            env: HashMap::new(),
            url: String::new(),
            headers: vec![],
            oauth: false,
            timeout_secs: None,
        });
        let path = PathBuf::from("/tmp/cdc.json");
        fs.write(&path, br#"{"mcpServers": {"github": {"command": "new"}}}"#)
            .unwrap();
        mgr.import_claude_desktop(&fs, &path).unwrap();
        let servers: Vec<_> = mgr.configured_servers().iter().filter(|s| s.name == "github").collect();
        assert_eq!(servers.len(), 1, "same-name import must replace, not duplicate");
        assert_eq!(servers[0].command, "new");
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
            timeout_secs: None,
        });
        mgr.save(&fs).unwrap();
        let loaded = McpManager::load(&fs);
        assert_eq!(loaded.configured_servers().len(), 1);
        assert_eq!(loaded.configured_servers()[0].name, "test");
    }

    #[test]
    fn expand_mcp_config_paths_splits_commas() {
        let paths = expand_mcp_config_paths(&[
            PathBuf::from("a.json,b.json"),
            PathBuf::from("c.json"),
        ]);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("a.json"),
                PathBuf::from("b.json"),
                PathBuf::from("c.json"),
            ]
        );
    }

    #[test]
    fn load_for_cli_strict_uses_only_cli_paths() {
        let fs = MockFs::new();
        fs.write(
            &McpManager::path(),
            br#"{"servers":[{"name":"user","command":"echo","args":[]}]}"#,
        )
        .unwrap();
        let cli_path = PathBuf::from("/tmp/cli-mcp.json");
        fs.write(
            &cli_path,
            br#"{"servers":[{"name":"cli","command":"cat","args":[]}]}"#,
        )
        .unwrap();

        let mgr = McpManager::load_for_cli(&fs, None, &[cli_path], true);
        assert_eq!(mgr.configured_servers().len(), 1);
        assert_eq!(mgr.configured_servers()[0].name, "cli");
    }

    #[test]
    fn load_for_cli_merges_user_project_and_cli_paths() {
        let fs = MockFs::new();
        fs.write(
            &McpManager::path(),
            br#"{"servers":[{"name":"user","command":"echo","args":[]}]}"#,
        )
        .unwrap();
        let project = PathBuf::from("/tmp/project");
        fs.write(
            &project.join(".mcp.json"),
            br#"{"servers":[{"name":"project","command":"ls","args":[]}]}"#,
        )
        .unwrap();
        let cli_path = PathBuf::from("/tmp/cli-mcp.json");
        fs.write(
            &cli_path,
            br#"{"servers":[{"name":"cli","command":"cat","args":[]}]}"#,
        )
        .unwrap();

        let mgr = McpManager::load_for_cli(&fs, Some(&project), &[cli_path], false);
        let names: Vec<_> = mgr.configured_servers().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["user", "project", "cli"]);
    }
}
