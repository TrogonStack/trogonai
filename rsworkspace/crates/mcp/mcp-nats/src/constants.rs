use std::time::Duration;

pub const DEFAULT_MCP_PREFIX: &str = "mcp";
pub const ENV_MCP_PREFIX: &str = "MCP_PREFIX";
pub const ENV_OPERATION_TIMEOUT_SECS: &str = "MCP_OPERATION_TIMEOUT_SECS";
pub const ENV_CONNECT_TIMEOUT_SECS: &str = "MCP_NATS_CONNECT_TIMEOUT_SECS";

pub const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;
pub const MIN_TIMEOUT_SECS: u64 = 1;

pub const DEFAULT_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
