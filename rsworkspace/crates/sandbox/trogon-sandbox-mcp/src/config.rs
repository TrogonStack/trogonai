use clap::Parser;
use trogon_sandbox_contract::{TenantId, TenantIdError};

/// The command line and environment of one server process.
#[derive(Debug, Clone, Parser)]
#[command(name = "trogon-sandbox-mcp", about = "MCP stdio server for sandbox lease operations")]
pub struct Config {
    #[arg(long, env = "SANDBOX_API_KEY")]
    pub api_key: Option<String>,

    #[arg(long, default_value = "nats://localhost:4222", env = "SANDBOX_NATS_URL")]
    pub nats_url: String,

    #[arg(long, env = "SANDBOX_TENANT", value_parser = parse_tenant)]
    pub tenant: TenantId,

    #[arg(long, default_value = "info", env = "SANDBOX_MCP_LOG_LEVEL")]
    pub log_level: String,

    /// Offer only tools whose name matches (exact, or a prefix ending in `*`). Repeatable.
    #[arg(long = "allow-tool", env = "SANDBOX_MCP_ALLOW_TOOLS", value_delimiter = ',')]
    pub allow_tool: Vec<String>,

    /// Never offer tools whose name matches; wins over `--allow-tool`. Repeatable.
    #[arg(long = "deny-tool", env = "SANDBOX_MCP_DENY_TOOLS", value_delimiter = ',')]
    pub deny_tool: Vec<String>,

    /// Refuse sandbox_run_command when the argv, joined by spaces, matches this regex. Repeatable.
    #[arg(long = "deny-command", env = "SANDBOX_MCP_DENY_COMMANDS", value_delimiter = '\n')]
    pub deny_command: Vec<String>,

    /// Permit sandbox_run_command only when the joined argv matches one of these regexes. Repeatable.
    #[arg(long = "allow-command", env = "SANDBOX_MCP_ALLOW_COMMANDS", value_delimiter = '\n')]
    pub allow_command: Vec<String>,

    /// Confine the file tools to these absolute sandbox directories. Relative paths resolve against the first root;
    /// `..` and symlinks below a root are refused. Repeatable.
    #[arg(long = "root", env = "SANDBOX_MCP_ROOTS", value_delimiter = ',')]
    pub root: Vec<String>,
}

fn parse_tenant(value: &str) -> Result<TenantId, TenantIdError> {
    TenantId::new(value)
}

#[cfg(test)]
mod tests;
