use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

const DEFAULT_PROXY_URL: &str = "http://localhost:8080";
const DEFAULT_MODEL: &str = "claude-opus-4-6";
const DEFAULT_MAX_ITERATIONS: u32 = 10;

/// Configuration for a single MCP server connection.
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    /// Short identifier used to prefix tool names (e.g. `"search"` → tool `mcp__search__my_tool`).
    pub name: String,
    /// HTTP endpoint of the MCP server (e.g. `"http://server:8080/mcp"`).
    pub url: String,
}

/// Parse the `MCP_SERVERS` env var: `"name1=url1,name2=url2"`.
pub(crate) fn parse_mcp_servers(s: &str) -> Vec<McpServerConfig> {
    s.split(',')
        .filter_map(|entry| {
            let mut parts = entry.splitn(2, '=');
            let name = parts.next()?.trim().to_string();
            let url = parts.next()?.trim().to_string();
            if name.is_empty() || url.is_empty() { None } else { Some(McpServerConfig { name, url }) }
        })
        .collect()
}

/// Runtime configuration for the agent, loaded from environment variables.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// NATS connection config (servers + auth).
    pub nats: NatsConfig,
    /// Base URL of the running `trogon-secret-proxy` instance.
    pub proxy_url: String,
    /// Opaque proxy token for Anthropic (e.g. `tok_anthropic_prod_xxx`).
    pub anthropic_token: String,
    /// Opaque proxy token for the GitHub API (e.g. `tok_github_prod_xxx`).
    pub github_token: String,
    /// Opaque proxy token for the Linear API (e.g. `tok_linear_prod_xxx`).
    pub linear_token: String,
    /// Anthropic model to use in the agentic loop.
    pub model: String,
    /// Maximum number of tool-use iterations before giving up.
    pub max_iterations: u32,
    /// JetStream stream name for GitHub events (default: `GITHUB`).
    pub github_stream_name: Option<String>,
    /// JetStream stream name for Linear events (default: `LINEAR`).
    pub linear_stream_name: Option<String>,
    /// GitHub repo owner for reading `.trogon/memory.md` in Linear handlers
    /// (e.g. `"my-org"`).  Not needed for PR handlers — they use the PR repo.
    pub memory_owner: Option<String>,
    /// GitHub repo name for reading `.trogon/memory.md` in Linear handlers
    /// (e.g. `"my-repo"`).
    pub memory_repo: Option<String>,
    /// Path of the memory file inside the repository (default: `.trogon/memory.md`).
    /// Overridable via `MEMORY_PATH` so the UI can configure per-automation files.
    pub memory_path: Option<String>,
    /// MCP servers to connect to at startup.
    /// Parsed from `MCP_SERVERS=name1=url1,name2=url2`.
    pub mcp_servers: Vec<McpServerConfig>,
}

impl AgentConfig {
    /// Build config from environment variables.
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        Self {
            nats: NatsConfig::from_env(env),
            proxy_url: env
                .var("PROXY_URL")
                .unwrap_or_else(|_| DEFAULT_PROXY_URL.to_string()),
            anthropic_token: env.var("ANTHROPIC_TOKEN").unwrap_or_default(),
            github_token: env.var("GITHUB_TOKEN").unwrap_or_default(),
            linear_token: env.var("LINEAR_TOKEN").unwrap_or_default(),
            model: env
                .var("AGENT_MODEL")
                .unwrap_or_else(|_| DEFAULT_MODEL.to_string()),
            max_iterations: env
                .var("AGENT_MAX_ITERATIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_MAX_ITERATIONS),
            github_stream_name: env.var("GITHUB_STREAM_NAME").ok(),
            linear_stream_name: env.var("LINEAR_STREAM_NAME").ok(),
            memory_owner: env.var("MEMORY_OWNER").ok(),
            memory_repo: env.var("MEMORY_REPO").ok(),
            memory_path: env.var("MEMORY_PATH").ok(),
            mcp_servers: env
                .var("MCP_SERVERS")
                .ok()
                .map(|s| parse_mcp_servers(&s))
                .unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;

    #[test]
    fn defaults_applied_when_env_is_empty() {
        let env = InMemoryEnv::new();
        let cfg = AgentConfig::from_env(&env);

        assert_eq!(cfg.proxy_url, DEFAULT_PROXY_URL);
        assert_eq!(cfg.model, DEFAULT_MODEL);
        assert_eq!(cfg.max_iterations, DEFAULT_MAX_ITERATIONS);
        assert!(cfg.anthropic_token.is_empty());
        assert!(cfg.github_token.is_empty());
        assert!(cfg.linear_token.is_empty());
        assert!(cfg.github_stream_name.is_none());
        assert!(cfg.linear_stream_name.is_none());
        assert!(cfg.memory_path.is_none());
    }

    #[test]
    fn env_values_override_defaults() {
        let env = InMemoryEnv::new();
        env.set("PROXY_URL", "http://proxy:9090");
        env.set("ANTHROPIC_TOKEN", "tok_anthropic_prod_abc");
        env.set("GITHUB_TOKEN", "tok_github_prod_xyz");
        env.set("LINEAR_TOKEN", "tok_linear_prod_qrs");
        env.set("AGENT_MODEL", "claude-haiku-4-5-20251001");
        env.set("AGENT_MAX_ITERATIONS", "5");

        let cfg = AgentConfig::from_env(&env);

        assert_eq!(cfg.proxy_url, "http://proxy:9090");
        assert_eq!(cfg.anthropic_token, "tok_anthropic_prod_abc");
        assert_eq!(cfg.github_token, "tok_github_prod_xyz");
        assert_eq!(cfg.linear_token, "tok_linear_prod_qrs");
        assert_eq!(cfg.model, "claude-haiku-4-5-20251001");
        assert_eq!(cfg.max_iterations, 5);
    }

    #[test]
    fn stream_names_override_defaults() {
        let env = InMemoryEnv::new();
        env.set("GITHUB_STREAM_NAME", "GH_EVENTS");
        env.set("LINEAR_STREAM_NAME", "LIN_EVENTS");

        let cfg = AgentConfig::from_env(&env);
        assert_eq!(cfg.github_stream_name.as_deref(), Some("GH_EVENTS"));
        assert_eq!(cfg.linear_stream_name.as_deref(), Some("LIN_EVENTS"));
    }

    #[test]
    fn invalid_max_iterations_falls_back_to_default() {
        let env = InMemoryEnv::new();
        env.set("AGENT_MAX_ITERATIONS", "not-a-number");

        let cfg = AgentConfig::from_env(&env);
        assert_eq!(cfg.max_iterations, DEFAULT_MAX_ITERATIONS);
    }

    #[test]
    fn memory_path_read_from_env() {
        let env = InMemoryEnv::new();
        env.set("MEMORY_PATH", ".trogon/custom-memory.md");

        let cfg = AgentConfig::from_env(&env);
        assert_eq!(cfg.memory_path.as_deref(), Some(".trogon/custom-memory.md"));
    }

    #[test]
    fn memory_path_is_none_when_not_set() {
        let env = InMemoryEnv::new();
        let cfg = AgentConfig::from_env(&env);
        assert!(cfg.memory_path.is_none());
    }

    #[test]
    fn parse_mcp_servers_valid_input() {
        let servers = parse_mcp_servers("search=http://localhost:3000,db=http://localhost:3001");
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].name, "search");
        assert_eq!(servers[0].url, "http://localhost:3000");
        assert_eq!(servers[1].name, "db");
        assert_eq!(servers[1].url, "http://localhost:3001");
    }

    #[test]
    fn parse_mcp_servers_url_with_equals_sign() {
        // URL contains '=' (e.g. query param) — splitn(2) must not split on it.
        let servers = parse_mcp_servers("search=http://host/mcp?token=abc123");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "search");
        assert_eq!(servers[0].url, "http://host/mcp?token=abc123");
    }

    #[test]
    fn parse_mcp_servers_empty_string_returns_empty_vec() {
        let servers = parse_mcp_servers("");
        assert!(servers.is_empty());
    }

    #[test]
    fn parse_mcp_servers_malformed_entry_ignored() {
        // "no-equals" has no '=' so it's skipped; valid entry still parsed.
        let servers = parse_mcp_servers("no-equals,valid=http://localhost:9000");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "valid");
        assert_eq!(servers[0].url, "http://localhost:9000");
    }

    #[test]
    fn parse_mcp_servers_whitespace_trimmed() {
        let servers = parse_mcp_servers(" search = http://localhost:3000 ");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "search");
        assert_eq!(servers[0].url, "http://localhost:3000");
    }

    #[test]
    fn parse_mcp_servers_empty_name_or_url_ignored() {
        // "=url" has empty name; "name=" has empty url — both skipped.
        let servers = parse_mcp_servers("=http://localhost,name=,valid=http://ok");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "valid");
    }

    #[test]
    fn mcp_servers_read_from_env() {
        let env = InMemoryEnv::new();
        env.set("MCP_SERVERS", "tool=http://localhost:8080");
        let cfg = AgentConfig::from_env(&env);
        assert_eq!(cfg.mcp_servers.len(), 1);
        assert_eq!(cfg.mcp_servers[0].name, "tool");
        assert_eq!(cfg.mcp_servers[0].url, "http://localhost:8080");
    }

    #[test]
    fn mcp_servers_empty_when_not_set() {
        let env = InMemoryEnv::new();
        let cfg = AgentConfig::from_env(&env);
        assert!(cfg.mcp_servers.is_empty());
    }
}
