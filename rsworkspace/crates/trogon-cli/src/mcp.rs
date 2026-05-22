//! MCP server config and stdio bridge lifecycle for the Trogon REPL.

use crate::fs::Fs;
use crate::stdio_mcp_bridge::StdioMcpBridge;
use agent_client_protocol::{McpServer, McpServerHttp};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

pub const MCP_CONFIG_PATH: &str = "~/.config/trogon/mcp.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

struct ActiveBridge {
    name: String,
    url: String,
    bridge: StdioMcpBridge,
}
pub struct McpManager {
    config: McpConfig,
    active: HashMap<String, Vec<ActiveBridge>>,
    pending: Vec<ActiveBridge>,
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
        Self { config, active: HashMap::new(), pending: Vec::new() }
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

    /// Spawn configured stdio MCP bridges; call [`Self::commit_pending`] after session.new.
    pub async fn spawn_pending(&mut self) -> Vec<McpServer> {
        self.pending.clear();
        let (servers, bridges) = self.spawn_bridges().await;
        self.pending = bridges;
        servers
    }

    pub fn commit_pending(&mut self, session_id: &str) {
        if self.pending.is_empty() {
            return;
        }
        self.active.insert(session_id.to_string(), std::mem::take(&mut self.pending));
    }

    async fn spawn_bridges(&self) -> (Vec<McpServer>, Vec<ActiveBridge>) {
        let mut bridges = Vec::new();
        let mut acp_servers = Vec::new();

        for cfg in &self.config.servers {
            let env: Vec<(String, String)> = cfg.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            match StdioMcpBridge::spawn(&cfg.command, &cfg.args, &env).await {
                Ok(bridge) => {
                    let url = bridge.url.clone();
                    acp_servers.push(McpServer::Http(McpServerHttp::new(cfg.name.clone(), url.clone())));
                    bridges.push(ActiveBridge {
                        name: cfg.name.clone(),
                        url,
                        bridge,
                    });
                }
                Err(e) => eprintln!("warning: MCP server `{}` failed to start: {e}", cfg.name),
            }
        }

        (acp_servers, bridges)
    }

    pub async fn shutdown_session(&mut self, session_id: &str) {
        if let Some(bridges) = self.active.remove(session_id) {
            for b in bridges {
                b.bridge.shutdown().await;
            }
        }
    }

    pub async fn shutdown_all(&mut self) {
        let ids: Vec<String> = self.active.keys().cloned().collect();
        for id in ids {
            self.shutdown_session(&id).await;
        }
        for b in std::mem::take(&mut self.pending) {
            b.bridge.shutdown().await;
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

    /// Parse `/mcp add <name> <command...>` tail into a config entry.
    pub fn parse_add_args(name: &str, rest: &str) -> Result<McpServerConfig, String> {
        if name.is_empty() {
            return Err("usage: /mcp add <name> <command> [args...]".into());
        }
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.is_empty() {
            return Err("usage: /mcp add <name> <command> [args...]".into());
        }
        Ok(McpServerConfig {
            name: name.to_string(),
            command: parts[0].to_string(),
            args: parts[1..].iter().map(|s| (*s).to_string()).collect(),
            env: HashMap::new(),
        })
    }
}

pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::mock::MockFs;

    #[test]
    fn parse_add_args_splits_command_and_args() {
        let cfg = McpManager::parse_add_args("github", "npx -y @modelcontextprotocol/server-github").unwrap();
        assert_eq!(cfg.name, "github");
        assert_eq!(cfg.command, "npx");
        assert_eq!(cfg.args, vec!["-y", "@modelcontextprotocol/server-github"]);
    }

    #[test]
    fn save_and_load_round_trip() {
        let fs = MockFs::new();
        let mut mgr = McpManager::load(&fs);
        mgr.add_server(McpServerConfig {
            name: "test".into(),
            command: "echo".into(),
            args: vec![],
            env: HashMap::new(),
        });
        mgr.save(&fs).unwrap();
        let loaded = McpManager::load(&fs);
        assert_eq!(loaded.configured_servers().len(), 1);
        assert_eq!(loaded.configured_servers()[0].name, "test");
    }
}
